//! Web search node (web-presence.md D361): `pnex-web-search`. The runtime
//! holds no provider and no key: each message calls
//! `POST /internal/flow/web-search` naming the deployed node; the backend
//! reads the options from the deployed config, queries the org provider
//! and answers the candidates. Port 0 results, port 1 failed.

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

use pnex_core::ontology::flow::FlowNodeRef;
use pnex_core::web_search::{WebSearchNodeReply, WebSearchNodeRequest, NODE_PORT_FAILED};

const NAME: &str = "pnex-web-search";
const FLOW_TOKEN_HEADER: &str = "x-pnex-flow-token";
/// The backend gives the provider 15 s.
const TIMEOUT: Duration = Duration::from_secs(30);
const UNAVAILABLE: &str = pnex_core::err_codes::WEB_SEARCH_UNAVAILABLE;

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

#[flow_node("pnex-web-search", red_name = "pnex-web-search")]
struct WebSearchNode {
    base: BaseFlowNodeState,
    node: FlowNodeRef,
    http: reqwest::Client,
    url: String,
    token: String,
}

impl WebSearchNode {
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
        Ok(Box::new(WebSearchNode {
            base: base_node,
            node: FlowNodeRef {
                org_id: c.pnex_org_id,
                flow_id: c.pnex_flow_id,
                version: c.pnex_version,
                node_id: c.pnex_node_id,
            },
            http,
            url: env("PNEX_FLOW_WEB_SEARCH_URL")?,
            token: env("PNEX_FLOW_WRITE_TOKEN")?,
        }))
    }

    /// The backend's answer, or a failure reply when it cannot be had.
    async fn call(&self, req: &WebSearchNodeRequest) -> WebSearchNodeReply {
        let refused = |code: String| WebSearchNodeReply {
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
                r.json::<WebSearchNodeReply>().await.unwrap_or_else(|e| {
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
        // The query: `msg.payload` when it is a string, else `msg.payload.query`.
        let query = {
            let m = msg.read().await;
            let payload = m
                .get("payload")
                .and_then(|v| serde_json::to_value(v).ok())
                .unwrap_or(Value::Null);
            match &payload {
                Value::String(s) => s.clone(),
                _ => payload["query"].as_str().unwrap_or_default().to_string(),
            }
        };
        let req = WebSearchNodeRequest {
            node: self.node.clone(),
            query: query.clone(),
        };
        let reply = self.call(&req).await;
        {
            let mut m = msg.write().await;
            let payload = serde_json::json!({
                "query": query,
                "provider": reply.provider,
                "results": reply.results,
            });
            m.set("payload".into(), Variant::from(payload));
            if let Some(code) = &reply.code {
                let error = serde_json::json!({ "code": code, "args": reply.args });
                m.set("error".into(), Variant::from(error));
            }
        }
        self.fan_out_one(
            Envelope {
                port: reply.port,
                msg,
            },
            cancel,
        )
        .await
    }
}

#[async_trait]
impl FlowNodeBehavior for WebSearchNode {
    fn get_base(&self) -> &BaseFlowNodeState {
        &self.base
    }

    async fn run(self: Arc<Self>, stop_token: CancellationToken) {
        while !stop_token.is_cancelled() {
            let cancel = stop_token.child_token();
            with_uow(
                self.as_ref(),
                cancel.child_token(),
                |node: &WebSearchNode, msg: MsgHandle| async move {
                    node.execute(msg, cancel.child_token()).await
                },
            )
            .await;
        }
    }
}
