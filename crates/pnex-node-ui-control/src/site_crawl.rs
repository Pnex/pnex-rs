//! Site crawl node (network-check.md D407): `pnex-site-crawl`. The
//! runtime reads no site: each message calls `POST /internal/flow/site-crawl`
//! naming the deployed node with the site and relevance rules of the
//! message; the backend crawls (minutes) and answers the relevant pages,
//! sent one message each on port 0 (the incoming message cloned), then the
//! summary on port 1, or a refusal on port 2.

use std::time::Duration;

use serde_json::Value;
use tokio_util::sync::CancellationToken;

use edgelink_core::runtime::context::*;
use edgelink_core::runtime::flow::*;
use edgelink_core::runtime::model::json::*;
use edgelink_core::runtime::model::*;
use edgelink_core::runtime::nodes::*;
use edgelink_core::Result;
use edgelink_macro::*;

use crate::proxy::{proxy_node_behavior, BackendProxy};
use pnex_core::browser_agent::Matcher;
use pnex_core::site_crawl::{
    SiteCrawlNodeReply, SiteCrawlNodeRequest, NODE_PORT_FAILED, NODE_PORT_PAGE, NODE_PORT_SUMMARY,
};

const NAME: &str = "pnex-site-crawl";
/// 2,000 pages at 2 s each, plus the sitemaps.
const TIMEOUT: Duration = Duration::from_secs(80 * 60);
const UNAVAILABLE: &str = pnex_core::err_codes::CRAWL_UNREACHABLE;

#[flow_node("pnex-site-crawl", red_name = "pnex-site-crawl")]
struct SiteCrawlNode {
    base: BaseFlowNodeState,
    proxy: BackendProxy,
}

impl SiteCrawlNode {
    fn build(
        _flow: &Flow,
        base_node: BaseFlowNodeState,
        config: &RedFlowNodeConfig,
        _options: Option<&config::Config>,
    ) -> Result<Box<dyn FlowNodeBehavior>> {
        Ok(Box::new(SiteCrawlNode {
            base: base_node,
            proxy: BackendProxy::from_config(NAME, config, "PNEX_FLOW_SITE_CRAWL_URL", TIMEOUT)?,
        }))
    }

    /// The backend's answer, or a failure reply when it cannot be had.
    async fn call(&self, req: &SiteCrawlNodeRequest) -> SiteCrawlNodeReply {
        self.proxy
            .call(self.name(), req, UNAVAILABLE, |code| SiteCrawlNodeReply {
                port: NODE_PORT_FAILED,
                code: Some(code),
                ..Default::default()
            })
            .await
    }

    async fn execute(&self, msg: MsgHandle, cancel: CancellationToken) -> Result<()> {
        let payload: Value = {
            let m = msg.read().await;
            m.get("payload")
                .and_then(|v| serde_json::to_value(v).ok())
                .unwrap_or(Value::Null)
        };
        let req = SiteCrawlNodeRequest {
            node: self.proxy.node.clone(),
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

proxy_node_behavior!(SiteCrawlNode);
