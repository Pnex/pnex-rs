//! Ingestion node (ingestion.md D317–D322): `pnex-ingest`. The runtime holds
//! no bytes and no credentials: each message calls
//! `POST /internal/flow/ingest` naming the deployed node; the backend runs
//! the step (config read from the deployed version), journals its manifest
//! and answers the next `msg.ingest`, `msg.manifest` and the output port —
//! 0 done, 1 error (`msg.error = {code, args}`), 2 unchanged.

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

use pnex_core::ingest::{IngestMsg, IngestReply, IngestRequest, PORT_ERROR};
use pnex_core::ontology::flow::FlowNodeRef;

const NAME: &str = "pnex-ingest";
const FLOW_TOKEN_HEADER: &str = "x-pnex-flow-token";
/// A download of 500 MB on a slow link, then its journal.
const TIMEOUT: Duration = Duration::from_secs(35 * 60);

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

#[flow_node("pnex-ingest", red_name = "pnex-ingest")]
struct IngestNode {
    base: BaseFlowNodeState,
    node: FlowNodeRef,
    http: reqwest::Client,
    url: String,
    token: String,
}

impl IngestNode {
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
        Ok(Box::new(IngestNode {
            base: base_node,
            node: FlowNodeRef {
                org_id: c.pnex_org_id,
                flow_id: c.pnex_flow_id,
                version: c.pnex_version,
                node_id: c.pnex_node_id,
            },
            http,
            url: env("PNEX_FLOW_INGEST_URL")?,
            token: env("PNEX_FLOW_WRITE_TOKEN")?,
        }))
    }

    /// The backend's answer, or an error reply when it cannot be had.
    async fn call(&self, req: &IngestRequest) -> IngestReply {
        let refused = |code: String| IngestReply {
            port: PORT_ERROR,
            msg: req.msg.clone(),
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
            Ok(r) if r.status().is_success() => r.json::<IngestReply>().await.unwrap_or_else(|e| {
                log::warn!("{NAME} [{}] : unreadable answer: {e}", self.name());
                refused(pnex_core::err_codes::INGEST_STAGING_UNAVAILABLE.into())
            }),
            Ok(r) => {
                let status = r.status();
                let body: Value = r.json().await.unwrap_or(Value::Null);
                log::warn!("{NAME} [{}] : refused ({status}): {body}", self.name());
                let code = body["code"]
                    .as_str()
                    .unwrap_or("ingest-staging-unavailable");
                refused(code.to_string())
            }
            Err(e) => {
                log::warn!("{NAME} [{}] : call failed: {e}", self.name());
                refused(pnex_core::err_codes::INGEST_STAGING_UNAVAILABLE.into())
            }
        }
    }

    async fn execute(&self, msg: MsgHandle, cancel: CancellationToken) -> Result<()> {
        let incoming: IngestMsg = {
            let m = msg.read().await;
            m.get("ingest")
                .and_then(|v| serde_json::to_value(v).ok())
                .and_then(|v| serde_json::from_value(v).ok())
                .unwrap_or_default()
        };
        let req = IngestRequest {
            node: self.node.clone(),
            msg: incoming,
        };
        let reply = self.call(&req).await;
        {
            let mut m = msg.write().await;
            let ingest = serde_json::to_value(&reply.msg).unwrap_or(Value::Null);
            m.set("ingest".into(), Variant::from(ingest));
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
impl FlowNodeBehavior for IngestNode {
    fn get_base(&self) -> &BaseFlowNodeState {
        &self.base
    }

    async fn run(self: Arc<Self>, stop_token: CancellationToken) {
        while !stop_token.is_cancelled() {
            let cancel = stop_token.child_token();
            with_uow(
                self.as_ref(),
                cancel.child_token(),
                |node: &IngestNode, msg: MsgHandle| async move {
                    node.execute(msg, cancel.child_token()).await
                },
            )
            .await;
        }
    }
}
