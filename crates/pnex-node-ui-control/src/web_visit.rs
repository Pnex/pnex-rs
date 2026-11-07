//! Website visit node (web-presence.md D364): `pnex-web-visit`. Each
//! message calls `POST /internal/flow/web-visit` naming the deployed node;
//! the backend reads the page and its legal notice and answers their text.
//! Port 0 read, port 1 failed.

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
use pnex_core::web_visit::{WebVisitNodeReply, WebVisitNodeRequest, NODE_PORT_FAILED};

const NAME: &str = "pnex-web-visit";
/// Three requests of 20 s at most, 2 s apart, plus the host queue.
const TIMEOUT: Duration = Duration::from_secs(90);
const UNAVAILABLE: &str = pnex_core::err_codes::WEB_VISIT_UNREACHABLE;

#[flow_node("pnex-web-visit", red_name = "pnex-web-visit")]
struct WebVisitNode {
    base: BaseFlowNodeState,
    proxy: BackendProxy,
}

impl WebVisitNode {
    fn build(
        _flow: &Flow,
        base_node: BaseFlowNodeState,
        config: &RedFlowNodeConfig,
        _options: Option<&config::Config>,
    ) -> Result<Box<dyn FlowNodeBehavior>> {
        Ok(Box::new(WebVisitNode {
            base: base_node,
            proxy: BackendProxy::from_config(NAME, config, "PNEX_FLOW_WEB_VISIT_URL", TIMEOUT)?,
        }))
    }

    /// The backend's answer, or a failure reply when it cannot be had.
    async fn call(&self, req: &WebVisitNodeRequest) -> WebVisitNodeReply {
        self.proxy
            .call(self.name(), req, UNAVAILABLE, |code| WebVisitNodeReply {
                port: NODE_PORT_FAILED,
                code: Some(code),
                ..Default::default()
            })
            .await
    }

    async fn execute(&self, msg: MsgHandle, cancel: CancellationToken) -> Result<()> {
        // The address: `msg.payload` when it is a string, else `msg.payload.url`.
        let url = {
            let m = msg.read().await;
            let payload = m
                .get("payload")
                .and_then(|v| serde_json::to_value(v).ok())
                .unwrap_or(Value::Null);
            match &payload {
                Value::String(s) => s.clone(),
                _ => payload["url"].as_str().unwrap_or_default().to_string(),
            }
        };
        let req = WebVisitNodeRequest {
            node: self.proxy.node.clone(),
            url: url.clone(),
        };
        let reply = self.call(&req).await;
        {
            let mut m = msg.write().await;
            let payload = serde_json::json!({
                "url": url,
                "final_url": reply.final_url,
                "status": reply.status,
                "title": reply.title,
                "text": reply.text,
                "legal_url": reply.legal_url,
                "legal_text": reply.legal_text,
                "screenshot_asset_id": reply.screenshot_asset_id,
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

proxy_node_behavior!(WebVisitNode);
