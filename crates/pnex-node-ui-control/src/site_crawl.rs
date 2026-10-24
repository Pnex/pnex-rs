//! Site crawl node (network-check.md D407): `pnex-site-crawl`. The
//! runtime reads no site: each message calls `POST /internal/flow/site-crawl`
//! naming the deployed node with the site and relevance rules of the
//! message; the backend crawls (minutes) and answers the relevant pages,
//! sent one message each on port 0 (the incoming message cloned), then the
//! summary on port 1, or a refusal on port 2.

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use serde::Deserialize;
use serde_json::Value;
use tokio_util::sync::CancellationToken;

use edgelink_core::runtime::context::*;
use edgelink_core::runtime::flow::*;
use edgelink_core::runtime::model::json::*;
use edgelink_core::runtime::model::*;
use edgelink_core::runtime::nodes::*;
use edgelink_core::{EdgelinkError, Result};
use edgelink_macro::*;

use pnex_core::browser_agent::Matcher;
use pnex_core::ontology::flow::FlowNodeRef;
use pnex_core::site_crawl::{
    SiteCrawlNodeReply, SiteCrawlNodeRequest, NODE_PORT_FAILED, NODE_PORT_PAGE, NODE_PORT_SUMMARY,
};

const NAME: &str = "pnex-site-crawl";
const FLOW_TOKEN_HEADER: &str = "x-pnex-flow-token";
/// 2,000 pages at 2 s each, plus the sitemaps.
const TIMEOUT: Duration = Duration::from_secs(80 * 60);
const UNAVAILABLE: &str = pnex_core::err_codes::CRAWL_UNREACHABLE;

#[derive(Debug, Deserialize)]
struct Common {
    #[serde(default)]
    pnex_node_id: String,
    #[serde(default)]
    pnex_flow_id: i64,
    #[serde(default)]
    pnex_version: i64,
    #[serde(default)]
    pnex_org_id: i64,
}

#[flow_node("pnex-site-crawl", red_name = "pnex-site-crawl")]
struct SiteCrawlNode {
    base: BaseFlowNodeState,
    node: FlowNodeRef,
    http: reqwest::Client,
    url: String,
    token: String,
}

impl SiteCrawlNode {
    fn build(
        _flow: &Flow,
        base_node: BaseFlowNodeState,
        config: &RedFlowNodeConfig,
        _options: Option<&config::Config>,
    ) -> Result<Box<dyn FlowNodeBehavior>> {
        let c = Common::deserialize(&config.rest)
            .map_err(|e| EdgelinkError::BadFlowsJson(format!("{NAME} : invalid config : {e}")))?;
        if c.pnex_org_id <= 0 || c.pnex_flow_id <= 0 {
            return Err(EdgelinkError::BadFlowsJson(format!(
                "{NAME} : pnex_org_id / pnex_flow_id missing from the artifact (redeploy the flow)"
            ))
            .into());
        }
        let env = |k: &str| {
            std::env::var(k).map_err(|_| {
                EdgelinkError::InvalidOperation(format!(
                    "{NAME} : {k} is not set in the runtime environment"
                ))
            })
        };
        let http = reqwest::Client::builder()
            .timeout(TIMEOUT)
            .build()
            .map_err(|e| EdgelinkError::InvalidOperation(format!("{NAME} : http client: {e}")))?;
        Ok(Box::new(SiteCrawlNode {
            base: base_node,
            node: FlowNodeRef {
                org_id: c.pnex_org_id,
                flow_id: c.pnex_flow_id,
                version: c.pnex_version,
                node_id: c.pnex_node_id,
            },
            http,
            url: env("PNEX_FLOW_SITE_CRAWL_URL")?,
            token: env("PNEX_FLOW_WRITE_TOKEN")?,
        }))
    }

    /// The backend's answer, or a failure reply when it cannot be had.
    async fn call(&self, req: &SiteCrawlNodeRequest) -> SiteCrawlNodeReply {
        let refused = |code: String| SiteCrawlNodeReply {
            port: NODE_PORT_FAILED,
            code: Some(code),
            ..Default::default()
        };
        let res = self
            .http
            .post(&self.url)
            .header(FLOW_TOKEN_HEADER, &self.token)
            .header(
                pnex_core::FLOW_WORKER_HEADER,
                pnex_core::flow_worker_fence().unwrap_or_default(),
            )
            .json(req)
            .send()
            .await;
        match res {
            Ok(r) if r.status().is_success() => {
                r.json::<SiteCrawlNodeReply>().await.unwrap_or_else(|e| {
                    log::warn!("{NAME} [{}] : unreadable answer: {e}", self.name());
                    refused(UNAVAILABLE.into())
                })
            }
            Ok(r) => {
                let status = r.status();
                let body: Value = r.json().await.unwrap_or(Value::Null);
                log::warn!("{NAME} [{}] : refused ({status}): {body}", self.name());
                refused(body["code"].as_str().unwrap_or(UNAVAILABLE).to_string())
            }
            Err(e) => {
                log::warn!("{NAME} [{}] : call failed: {e}", self.name());
                refused(UNAVAILABLE.into())
            }
        }
    }

    async fn execute(&self, msg: MsgHandle, cancel: CancellationToken) -> Result<()> {
        let payload: Value = {
            let m = msg.read().await;
            m.get("payload")
                .and_then(|v| serde_json::to_value(v).ok())
                .unwrap_or(Value::Null)
        };
        let req = SiteCrawlNodeRequest {
            node: self.node.clone(),
            url: payload["url"].as_str().unwrap_or_default().to_string(),
            matchers: serde_json::from_value::<Vec<Matcher>>(payload["match"].clone())
                .unwrap_or_default(),
            link_to: payload["link_to"].as_str().map(str::to_string),
        };
        let reply = self.call(&req).await;
        if reply.port == NODE_PORT_FAILED {
            {
                let mut m = msg.write().await;
                let error = serde_json::json!({ "code": reply.code, "args": reply.args });
                m.set("error".into(), Variant::from(error));
            }
            return self
                .fan_out_one(
                    Envelope {
                        port: NODE_PORT_FAILED,
                        msg,
                    },
                    cancel,
                )
                .await;
        }
        for page in &reply.pages {
            let out = msg.deep_clone(false).await;
            {
                let mut m = out.write().await;
                let p = serde_json::to_value(page).unwrap_or(Value::Null);
                m.set("payload".into(), Variant::from(p));
            }
            self.fan_out_one(
                Envelope {
                    port: NODE_PORT_PAGE,
                    msg: out,
                },
                cancel.child_token(),
            )
            .await?;
        }
        {
            let mut m = msg.write().await;
            let summary = serde_json::to_value(&reply.summary).unwrap_or(Value::Null);
            m.set("payload".into(), Variant::from(summary));
        }
        self.fan_out_one(
            Envelope {
                port: NODE_PORT_SUMMARY,
                msg,
            },
            cancel,
        )
        .await
    }
}

#[async_trait]
impl FlowNodeBehavior for SiteCrawlNode {
    fn get_base(&self) -> &BaseFlowNodeState {
        &self.base
    }

    async fn run(self: Arc<Self>, stop_token: CancellationToken) {
        while !stop_token.is_cancelled() {
            let cancel = stop_token.child_token();
            with_uow(
                self.as_ref(),
                cancel.child_token(),
                |node: &SiteCrawlNode, msg: MsgHandle| async move {
                    node.execute(msg, cancel.child_token()).await
                },
            )
            .await;
        }
    }
}
