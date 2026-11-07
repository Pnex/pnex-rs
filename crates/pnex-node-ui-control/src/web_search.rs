//! Web search node (web-presence.md D361): `pnex-web-search`. The runtime
//! holds no provider and no key: each message calls
//! `POST /internal/flow/web-search` naming the deployed node; the backend
//! reads the options from the deployed config, queries the org provider
//! and answers the candidates. Port 0 results, port 1 failed.

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
use pnex_core::web_search::{WebSearchNodeReply, WebSearchNodeRequest, NODE_PORT_FAILED};

const NAME: &str = "pnex-web-search";
/// The backend gives the provider 15 s.
const TIMEOUT: Duration = Duration::from_secs(30);
const UNAVAILABLE: &str = pnex_core::err_codes::WEB_SEARCH_UNAVAILABLE;

#[flow_node("pnex-web-search", red_name = "pnex-web-search")]
struct WebSearchNode {
    base: BaseFlowNodeState,
    proxy: BackendProxy,
}

impl WebSearchNode {
    fn build(
        _flow: &Flow,
        base_node: BaseFlowNodeState,
        config: &RedFlowNodeConfig,
        _options: Option<&config::Config>,
    ) -> Result<Box<dyn FlowNodeBehavior>> {
        Ok(Box::new(WebSearchNode {
            base: base_node,
            proxy: BackendProxy::from_config(NAME, config, "PNEX_FLOW_WEB_SEARCH_URL", TIMEOUT)?,
        }))
    }

    /// The backend's answer, or a failure reply when it cannot be had.
    async fn call(&self, req: &WebSearchNodeRequest) -> WebSearchNodeReply {
        self.proxy
            .call(self.name(), req, UNAVAILABLE, |code| WebSearchNodeReply {
                port: NODE_PORT_FAILED,
                code: Some(code),
                ..Default::default()
            })
            .await
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
            node: self.proxy.node.clone(),
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

proxy_node_behavior!(WebSearchNode);
