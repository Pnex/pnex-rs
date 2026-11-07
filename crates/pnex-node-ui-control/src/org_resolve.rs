//! Company name → SIREN node (account-mapping.md D332): `pnex-org-resolve`.
//! Each message calls `POST /internal/flow/org-resolve` naming the deployed
//! node; the backend reads the thresholds from the deployed config, asks
//! the company register and answers scored candidates. Port 0 matched,
//! port 1 to review, port 2 failed.

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
use pnex_core::org_resolve::{
    OrgQuery, OrgResolveNodeReply, OrgResolveNodeRequest, NODE_PORT_FAILED,
};

const NAME: &str = "pnex-org-resolve";
/// The backend gives the register 15 s, twice at most.
const TIMEOUT: Duration = Duration::from_secs(40);
const UNAVAILABLE: &str = pnex_core::err_codes::ORG_RESOLVE_UNAVAILABLE;

#[flow_node("pnex-org-resolve", red_name = "pnex-org-resolve")]
struct OrgResolveNode {
    base: BaseFlowNodeState,
    proxy: BackendProxy,
}

impl OrgResolveNode {
    fn build(
        _flow: &Flow,
        base_node: BaseFlowNodeState,
        config: &RedFlowNodeConfig,
        _options: Option<&config::Config>,
    ) -> Result<Box<dyn FlowNodeBehavior>> {
        Ok(Box::new(OrgResolveNode {
            base: base_node,
            proxy: BackendProxy::from_config(NAME, config, "PNEX_FLOW_ORG_RESOLVE_URL", TIMEOUT)?,
        }))
    }

    /// The backend's answer, or a failure reply when it cannot be had.
    async fn call(&self, req: &OrgResolveNodeRequest) -> OrgResolveNodeReply {
        self.proxy
            .call(self.name(), req, UNAVAILABLE, |code| OrgResolveNodeReply {
                port: NODE_PORT_FAILED,
                code: Some(code),
                ..Default::default()
            })
            .await
    }

    async fn execute(&self, msg: MsgHandle, cancel: CancellationToken) -> Result<()> {
        // The name: `msg.payload` when it is a string, else
        // `msg.payload.{name, city, postcode}`.
        let query = {
            let m = msg.read().await;
            let payload = m
                .get("payload")
                .and_then(|v| serde_json::to_value(v).ok())
                .unwrap_or(Value::Null);
            let text = |k: &str| match &payload[k] {
                Value::String(s) => s.clone(),
                Value::Number(n) => n.to_string(),
                _ => String::new(),
            };
            match &payload {
                Value::String(s) => OrgQuery {
                    name: s.clone(),
                    ..Default::default()
                },
                _ => OrgQuery {
                    name: text("name"),
                    city: text("city"),
                    postcode: text("postcode"),
                },
            }
        };
        let req = OrgResolveNodeRequest {
            node: self.proxy.node.clone(),
            query: query.clone(),
        };
        let reply = self.call(&req).await;
        {
            let mut m = msg.write().await;
            let payload = serde_json::json!({
                "query": query,
                "siren": reply.siren,
                "candidates": reply.candidates,
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

proxy_node_behavior!(OrgResolveNode);
