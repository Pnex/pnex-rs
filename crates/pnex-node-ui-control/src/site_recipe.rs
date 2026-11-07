//! Site navigation rule node (site-recipes.md D327): `pnex-site-recipe`.
//! The runtime holds no rule and no browser: each message calls
//! `POST /internal/flow/site-recipe` naming the deployed node; the backend
//! plays the rule read from the deployed config, journals the run and
//! answers the extracts (`msg.payload`), the manifest (`msg.manifest`) and
//! the output port — 0 passed, 1 failed, 2 broke (failed after a pass).

use std::collections::BTreeMap;
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
use pnex_core::site_recipe::{SiteRecipeNodeReply, SiteRecipeNodeRequest, NODE_PORT_FAILED};

const NAME: &str = "pnex-site-recipe";
/// A run lasts 10 minutes at most, plus waiting for a browser slot.
const TIMEOUT: Duration = Duration::from_secs(20 * 60);
const UNAVAILABLE: &str = pnex_core::err_codes::SITE_RECIPE_UNAVAILABLE;

#[flow_node("pnex-site-recipe", red_name = "pnex-site-recipe")]
struct SiteRecipeNode {
    base: BaseFlowNodeState,
    proxy: BackendProxy,
}

impl SiteRecipeNode {
    fn build(
        _flow: &Flow,
        base_node: BaseFlowNodeState,
        config: &RedFlowNodeConfig,
        _options: Option<&config::Config>,
    ) -> Result<Box<dyn FlowNodeBehavior>> {
        Ok(Box::new(SiteRecipeNode {
            base: base_node,
            proxy: BackendProxy::from_config(NAME, config, "PNEX_FLOW_SITE_RECIPE_URL", TIMEOUT)?,
        }))
    }

    /// The backend's answer, or a failure reply when it cannot be had.
    async fn call(&self, req: &SiteRecipeNodeRequest) -> SiteRecipeNodeReply {
        self.proxy
            .call(self.name(), req, UNAVAILABLE, |code| SiteRecipeNodeReply {
                port: NODE_PORT_FAILED,
                code: Some(code),
                ..Default::default()
            })
            .await
    }

    async fn execute(&self, msg: MsgHandle, cancel: CancellationToken) -> Result<()> {
        let params: BTreeMap<String, String> = {
            let m = msg.read().await;
            m.get("params")
                .and_then(|v| serde_json::to_value(v).ok())
                .and_then(|v| v.as_object().cloned())
                .map(|o| {
                    o.into_iter()
                        .filter_map(|(k, v)| match v {
                            Value::String(s) => Some((k, s)),
                            Value::Number(n) => Some((k, n.to_string())),
                            _ => None,
                        })
                        .collect()
                })
                .unwrap_or_default()
        };
        let req = SiteRecipeNodeRequest {
            node: self.proxy.node.clone(),
            params,
        };
        let reply = self.call(&req).await;
        {
            let mut m = msg.write().await;
            let payload = serde_json::to_value(&reply.payload).unwrap_or(Value::Null);
            m.set("payload".into(), Variant::from(payload));
            m.set("manifest".into(), Variant::from(reply.manifest.clone()));
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

proxy_node_behavior!(SiteRecipeNode);
