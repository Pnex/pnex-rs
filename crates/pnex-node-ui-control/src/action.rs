//! `pnex-action-trigger` — event source of one ontology action (D183,
//! `docs/architecture/ontology.md`).
//!
//! The backend checks an invocation, then publishes an `ActionEvent` on
//! `pnex:act:v1:{org}`. This node subscribes to the org channel and emits one
//! message per invocation of its type and action (`payload` = parameters,
//! `topic` = action key, `msg.object`, `msg.action`). No replay: an
//! invocation is an event, not a state.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use futures_util::StreamExt;
use serde::Deserialize;
use tokio_util::sync::CancellationToken;

use edgelink_core::runtime::context::*;
use edgelink_core::runtime::flow::*;
use edgelink_core::runtime::model::json::*;
use edgelink_core::runtime::model::*;
use edgelink_core::runtime::nodes::*;
use edgelink_core::{EdgelinkError, Result};
use edgelink_macro::*;

use pnex_core::ontology::action::{action_channel, ActionEvent, ActionTriggerConfig};

const NODE: &str = "pnex-action-trigger";
const BACKOFF_MAX_SECS: u64 = 30;

#[derive(Debug, Deserialize)]
struct NodeConfig {
    // No `serde(flatten)`: it breaks numbers under `arbitrary_precision`.
    #[serde(default)]
    object_type: String,
    #[serde(default)]
    action: String,
    /// Stamped by the deploy projection.
    #[serde(default)]
    pnex_org_id: i64,
}

#[flow_node("pnex-action-trigger", red_name = "pnex-action-trigger")]
struct ActionTriggerNode {
    base: BaseFlowNodeState,
    config: NodeConfig,
    trigger: ActionTriggerConfig,
    client: redis::Client,
}

impl NodeConfig {
    fn trigger(&self) -> ActionTriggerConfig {
        ActionTriggerConfig {
            object_type: self.object_type.clone(),
            action: self.action.clone(),
        }
    }
}

/// Message body of one invocation.
fn body_of(ev: &ActionEvent) -> BTreeMap<String, Variant> {
    let mut body = BTreeMap::new();
    body.insert(
        "payload".to_string(),
        Variant::from(serde_json::Value::Object(ev.params.clone())),
    );
    body.insert("topic".to_string(), Variant::from(ev.action.clone()));
    body.insert(
        "object".to_string(),
        Variant::from(serde_json::json!({
            "id": ev.object_id.to_string(),
            "type": ev.object_type,
            "title": ev.object_title,
        })),
    );
    body.insert(
        "action".to_string(),
        Variant::from(serde_json::json!({
            "invocation_id": ev.invocation_id.to_string(),
            "key": ev.action,
            "by": ev.by,
            "ts_ms": ev.ts_ms,
        })),
    );
    body
}

impl ActionTriggerNode {
    fn build(
        _flow: &Flow,
        base_node: BaseFlowNodeState,
        config: &RedFlowNodeConfig,
        _options: Option<&config::Config>,
    ) -> Result<Box<dyn FlowNodeBehavior>> {
        let cfg = NodeConfig::deserialize(&config.rest)
            .map_err(|e| EdgelinkError::BadFlowsJson(format!("{NODE} : invalid config : {e}")))?;
        if let Some((code, message)) = cfg.trigger().check_deployable() {
            return Err(EdgelinkError::BadFlowsJson(format!("{NODE} [{code}] : {message}")).into());
        }
        if cfg.pnex_org_id <= 0 {
            return Err(EdgelinkError::BadFlowsJson(format!(
                "{NODE} : pnex_org_id missing from the artifact (redeploy the flow)"
            ))
            .into());
        }
        let url = std::env::var("VALKEY_URL")
            .ok()
            .filter(|u| !u.trim().is_empty())
            .ok_or_else(|| {
                EdgelinkError::InvalidOperation(format!(
                    "{NODE} [action_bus_unavailable] : VALKEY_URL is not set in the runtime environment"
                ))
            })?;
        let client = redis::Client::open(url.as_str()).map_err(|e| {
            EdgelinkError::InvalidOperation(format!("{NODE} : invalid VALKEY_URL: {e}"))
        })?;
        Ok(Box::new(ActionTriggerNode {
            base: base_node,
            trigger: cfg.trigger(),
            config: cfg,
            client,
        }))
    }

    /// One subscription session: returns when the connection drops (the
    /// caller reconnects) or the node stops.
    async fn session(&self, stop: &CancellationToken) -> std::result::Result<(), String> {
        let channel = action_channel(self.config.pnex_org_id);
        let mut pubsub = self
            .client
            .get_async_pubsub()
            .await
            .map_err(|e| format!("valkey connect failed: {e}"))?;
        pubsub
            .subscribe(&channel)
            .await
            .map_err(|e| format!("valkey subscribe failed: {e}"))?;
        log::info!("{NODE} [{}] : subscribed to {channel}", self.name());
        let mut stream = pubsub.on_message();
        loop {
            let msg = tokio::select! {
                _ = stop.cancelled() => return Ok(()),
                m = stream.next() => match m {
                    Some(m) => m,
                    None => return Err("valkey subscription closed".into()),
                },
            };
            let Ok(raw) = msg.get_payload::<String>() else {
                continue;
            };
            let Ok(ev) = serde_json::from_str::<ActionEvent>(&raw) else {
                continue;
            };
            if !ev.matches(&self.trigger) {
                continue;
            }
            let envelope = Envelope {
                port: 0,
                msg: MsgHandle::with_properties(body_of(&ev)),
            };
            if let Err(e) = self.fan_out_one(envelope, stop.child_token()).await {
                log::warn!("{NODE} [{}] : fan-out failed: {e}", self.name());
            }
        }
    }
}

#[async_trait]
impl FlowNodeBehavior for ActionTriggerNode {
    fn get_base(&self) -> &BaseFlowNodeState {
        &self.base
    }

    async fn run(self: Arc<Self>, stop_token: CancellationToken) {
        let mut backoff = 1u64;
        while !stop_token.is_cancelled() {
            match self.session(&stop_token).await {
                Ok(()) => break,
                Err(e) => {
                    log::warn!("{NODE} [{}] : {e} — retry in {backoff} s", self.name());
                    tokio::select! {
                        _ = stop_token.cancelled() => break,
                        _ = tokio::time::sleep(Duration::from_secs(backoff)) => {}
                    }
                    backoff = (backoff * 2).min(BACKOFF_MAX_SECS);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use uuid::Uuid;

    #[test]
    fn message_carries_params_object_and_invocation() {
        let mut params = serde_json::Map::new();
        params.insert("reason".into(), serde_json::json!("noise"));
        let ev = ActionEvent {
            invocation_id: Uuid::from_u128(7),
            object_id: Uuid::from_u128(1),
            object_type: "pump".into(),
            object_title: "P12".into(),
            action: "start_maintenance".into(),
            params,
            by: Some("alice".into()),
            ts_ms: 1_790_000_000_000,
        };
        let body = body_of(&ev);
        let payload = serde_json::to_value(&body["payload"]).unwrap();
        assert_eq!(payload["reason"], "noise");
        assert_eq!(
            serde_json::to_value(&body["topic"]).unwrap(),
            "start_maintenance"
        );
        let object = serde_json::to_value(&body["object"]).unwrap();
        assert_eq!(object["type"], "pump");
        assert_eq!(object["title"], "P12");
        let action = serde_json::to_value(&body["action"]).unwrap();
        assert_eq!(action["invocation_id"], Uuid::from_u128(7).to_string());
        assert_eq!(action["by"], "alice");
    }

    #[test]
    fn config_reads_the_projection() {
        let raw = serde_json::json!({
            "type": "pnex-action-trigger",
            "object_type": "pump",
            "action": "start_maintenance",
            "pnex_org_id": 3,
            "pnex_node_id": "n1",
        });
        let cfg = NodeConfig::deserialize(&raw).unwrap();
        assert_eq!(cfg.pnex_org_id, 3);
        assert!(cfg.trigger().check_deployable().is_none());
    }
}
