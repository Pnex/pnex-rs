//! Site navigation rule node (site-recipes.md D327): `pnex-site-recipe`.
//! The runtime holds no rule and no browser: each message calls
//! `POST /internal/flow/site-recipe` naming the deployed node; the backend
//! plays the rule read from the deployed config, journals the run and
//! answers the extracts (`msg.payload`), the manifest (`msg.manifest`) and
//! the output port — 0 passed, 1 failed, 2 broke (failed after a pass).

use std::collections::BTreeMap;
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
use pnex_core::site_recipe::{SiteRecipeNodeReply, SiteRecipeNodeRequest, NODE_PORT_FAILED};

const NAME: &str = "pnex-site-recipe";
const FLOW_TOKEN_HEADER: &str = "x-pnex-flow-token";
/// A run lasts 10 minutes at most, plus waiting for a browser slot.
const TIMEOUT: Duration = Duration::from_secs(20 * 60);
const UNAVAILABLE: &str = pnex_core::err_codes::SITE_RECIPE_UNAVAILABLE;

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

#[flow_node("pnex-site-recipe", red_name = "pnex-site-recipe")]
struct SiteRecipeNode {
    base: BaseFlowNodeState,
    node: FlowNodeRef,
    http: reqwest::Client,
    url: String,
    token: String,
}

impl SiteRecipeNode {
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
        Ok(Box::new(SiteRecipeNode {
            base: base_node,
            node: FlowNodeRef {
                org_id: c.pnex_org_id,
                flow_id: c.pnex_flow_id,
                version: c.pnex_version,
                node_id: c.pnex_node_id,
            },
            http,
            url: env("PNEX_FLOW_SITE_RECIPE_URL")?,
            token: env("PNEX_FLOW_WRITE_TOKEN")?,
        }))
    }

    /// The backend's answer, or a failure reply when it cannot be had.
    async fn call(&self, req: &SiteRecipeNodeRequest) -> SiteRecipeNodeReply {
        let refused = |code: String| SiteRecipeNodeReply {
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
                r.json::<SiteRecipeNodeReply>().await.unwrap_or_else(|e| {
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
            node: self.node.clone(),
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

#[async_trait]
impl FlowNodeBehavior for SiteRecipeNode {
    fn get_base(&self) -> &BaseFlowNodeState {
        &self.base
    }

    async fn run(self: Arc<Self>, stop_token: CancellationToken) {
        while !stop_token.is_cancelled() {
            let cancel = stop_token.child_token();
            with_uow(
                self.as_ref(),
                cancel.child_token(),
                |node: &SiteRecipeNode, msg: MsgHandle| async move {
                    node.execute(msg, cancel.child_token()).await
                },
            )
            .await;
        }
    }
}
