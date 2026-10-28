//! `pnex-display` node (probe) — passthrough + publication to the engine's debug
//! channel with the raw canvas id. The backend collects these events (runtime
//! stdout → in-memory ring) for the debug panel and the live badge
//! under the node in the editor.
//!
//! Safeguards:
//! - `pnex_node_id` **required** at build (stamped by the projection) — an
//!   artefact that does not carry it is rejected at deployment (fail-loud);
//! - passthrough intact: the message continues on its way unchanged.

use std::sync::Arc;

use async_trait::async_trait;
use serde::Deserialize;
use tokio_util::sync::CancellationToken;

use edgelink_core::runtime::context::*;
use edgelink_core::runtime::flow::*;
use edgelink_core::runtime::model::json::*;
use edgelink_core::runtime::model::*;
use edgelink_core::runtime::nodes::*;
use edgelink_core::{EdgelinkError, Result};
use edgelink_macro::*;

#[derive(Debug, Deserialize)]
struct DisplayNodeConfig {
    /// Stamped by the projection = the node's canvas id (`"n3"`) — the
    /// attachment key of the panel and badge. Never entered by the user.
    /// (The other traceability stamps — pnex_flow_id/version/org_id — are
    /// present in the artefact but ignored here: the node does not need them.)
    pnex_node_id: String,
}

#[flow_node("pnex-display", red_name = "pnex-display")]
struct PnexDisplayNode {
    base: BaseFlowNodeState,
    config: DisplayNodeConfig,
}

impl PnexDisplayNode {
    fn build(
        _flow: &Flow,
        base_node: BaseFlowNodeState,
        config: &RedFlowNodeConfig,
        _options: Option<&config::Config>,
    ) -> Result<Box<dyn FlowNodeBehavior>> {
        let cfg = DisplayNodeConfig::deserialize(&config.rest).map_err(|e| {
            EdgelinkError::BadFlowsJson(format!("pnex-display: invalid config: {e}"))
        })?;
        if cfg.pnex_node_id.trim().is_empty() {
            return Err(EdgelinkError::BadFlowsJson(
                "pnex-display: pnex_node_id missing from the artefact (redeploy the flow)".into(),
            )
            .into());
        }
        Ok(Box::new(PnexDisplayNode {
            base: base_node,
            config: cfg,
        }))
    }

    /// Passthrough + publication: the value (payload if present, otherwise the
    /// whole message) goes on the debug channel with the raw canvas id, then
    /// the original message continues on its way unchanged.
    async fn execute(&self, msg: MsgHandle, cancel: CancellationToken) -> Result<()> {
        let (captured, topic, msgid) = {
            let m = msg.read().await;
            let captured: serde_json::Value = match m.get("payload") {
                Some(v) => serde_json::to_value(v).unwrap_or(serde_json::Value::Null),
                None => serde_json::to_value(&*m).unwrap_or(serde_json::Value::Null),
            };
            let topic = m.get("topic").and_then(|t| t.as_str()).map(str::to_string);
            let msgid = m.get("_msgid").and_then(|v| v.as_str()).map(str::to_string);
            (captured, topic, msgid)
        };

        if let Some(engine) = self.engine() {
            let path = self
                .flow()
                .map(|f| f.get_path())
                .unwrap_or_else(|| "global".to_string());
            let name = self.name();
            engine
                .debug_channel()
                .send(edgelink_core::runtime::debug_channel::DebugMessage {
                    // RAW canvas id (not the engine hash): direct match with
                    // the editor node for the live badge.
                    id: self.config.pnex_node_id.clone(),
                    name: if name.is_empty() {
                        None
                    } else {
                        Some(name.to_string())
                    },
                    msg: captured,
                    property: Some("payload".into()),
                    format: Some("pnex-display".into()),
                    path,
                    topic,
                    timestamp: Some(
                        std::time::SystemTime::now()
                            .duration_since(std::time::UNIX_EPOCH)
                            .map(|d| d.as_millis() as i64)
                            .unwrap_or(0),
                    ),
                    msgid,
                });
        } else {
            log::warn!(
                "pnex-display [{}]: engine unreachable, no publication",
                self.name()
            );
        }

        // Passthrough intact.
        self.fan_out_one(Envelope { port: 0, msg }, cancel).await
    }
}

#[async_trait]
impl FlowNodeBehavior for PnexDisplayNode {
    fn get_base(&self) -> &BaseFlowNodeState {
        &self.base
    }

    async fn run(self: Arc<Self>, stop_token: CancellationToken) {
        while !stop_token.is_cancelled() {
            let cancel = stop_token.child_token();
            with_uow(
                self.as_ref(),
                cancel.child_token(),
                |node: &PnexDisplayNode, msg: MsgHandle| async move {
                    match node.execute(msg.clone(), cancel.child_token()).await {
                        Ok(()) => Ok(()),
                        Err(e) => {
                            log::warn!("pnex-display [{}]: message rejected: {e}", node.name());
                            Err(e)
                        }
                    }
                },
            )
            .await;
        }
    }
}
