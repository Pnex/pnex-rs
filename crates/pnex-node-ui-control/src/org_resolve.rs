//! Company name → SIREN node (account-mapping.md D332): `pnex-org-resolve`.
//! Each message calls `POST /internal/flow/org-resolve` naming the deployed
//! node; the backend reads the thresholds from the deployed config, asks
//! the company register and answers scored candidates. Port 0 matched,
//! port 1 to review, port 2 failed.

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
use pnex_core::org_resolve::{
    OrgQuery, OrgResolveNodeReply, OrgResolveNodeRequest, NODE_PORT_FAILED,
};

const NAME: &str = "pnex-org-resolve";
const FLOW_TOKEN_HEADER: &str = "x-pnex-flow-token";
/// The backend gives the register 15 s, twice at most.
const TIMEOUT: Duration = Duration::from_secs(40);
const UNAVAILABLE: &str = pnex_core::err_codes::ORG_RESOLVE_UNAVAILABLE;

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

#[flow_node("pnex-org-resolve", red_name = "pnex-org-resolve")]
struct OrgResolveNode {
    base: BaseFlowNodeState,
    node: FlowNodeRef,
    http: reqwest::Client,
    url: String,
    token: String,
}

impl OrgResolveNode {
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
        Ok(Box::new(OrgResolveNode {
            base: base_node,
            node: FlowNodeRef {
                org_id: c.pnex_org_id,
                flow_id: c.pnex_flow_id,
                version: c.pnex_version,
                node_id: c.pnex_node_id,
            },
            http,
            url: env("PNEX_FLOW_ORG_RESOLVE_URL")?,
            token: env("PNEX_FLOW_WRITE_TOKEN")?,
        }))
    }

    /// The backend's answer, or a failure reply when it cannot be had.
    async fn call(&self, req: &OrgResolveNodeRequest) -> OrgResolveNodeReply {
        let refused = |code: String| OrgResolveNodeReply {
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
                r.json::<OrgResolveNodeReply>().await.unwrap_or_else(|e| {
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
            node: self.node.clone(),
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

#[async_trait]
impl FlowNodeBehavior for OrgResolveNode {
    fn get_base(&self) -> &BaseFlowNodeState {
        &self.base
    }

    async fn run(self: Arc<Self>, stop_token: CancellationToken) {
        while !stop_token.is_cancelled() {
            let cancel = stop_token.child_token();
            with_uow(
                self.as_ref(),
                cancel.child_token(),
                |node: &OrgResolveNode, msg: MsgHandle| async move {
                    node.execute(msg, cancel.child_token()).await
                },
            )
            .await;
        }
    }
}
