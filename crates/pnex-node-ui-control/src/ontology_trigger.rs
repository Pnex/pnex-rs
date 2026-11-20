//! `pnex-ontology-trigger`: event source of one object type
//! (operational-platform.md D998, pipelines.md D999).
//!
//! The server publishes object, link and decided-proposal events on
//! `pnex:onto:v1:{org}`. This node subscribes to the org channel and emits one
//! message per event it matches. The message names the object, never its values:
//! the flow reads them back. No replay: a frame missed while the flow is stopped
//! is gone.

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

use pnex_core::ontology::trigger::{ontology_channel, OntologyEvent, OntologyTriggerConfig};

const NODE: &str = "pnex-ontology-trigger";
const BACKOFF_MAX_SECS: u64 = 30;

#[derive(Debug, Deserialize)]
struct NodeConfig {
    // No `serde(flatten)`: it breaks numbers under `arbitrary_precision`.
    #[serde(default)]
    object_type: String,
    #[serde(default)]
    on: Vec<String>,
    #[serde(default)]
    properties: Vec<String>,
    #[serde(default)]
    link_types: Vec<String>,
    #[serde(default)]
    origin: String,
    #[serde(default)]
    skip_own: bool,
    /// Stamped by the deploy projection.
    #[serde(default)]
    pnex_org_id: i64,
    #[serde(default)]
    pnex_flow_id: i64,
}

impl NodeConfig {
    fn trigger(&self) -> OntologyTriggerConfig {
        OntologyTriggerConfig {
            object_type: self.object_type.clone(),
            on: self.on.clone(),
            properties: self.properties.clone(),
            link_types: self.link_types.clone(),
            origin: self.origin.clone(),
            skip_own: self.skip_own,
        }
    }
}

#[flow_node("pnex-ontology-trigger", red_name = "pnex-ontology-trigger")]
struct OntologyTriggerNode {
    base: BaseFlowNodeState,
    config: NodeConfig,
    trigger: OntologyTriggerConfig,
    /// Source prefix of the flow's own writes.
    own_source: String,
    client: redis::Client,
}

/// Message body of one event.
fn body_of(ev: &OntologyEvent) -> BTreeMap<String, Variant> {
    let mut body = BTreeMap::new();
    body.insert(
        "payload".to_string(),
        Variant::from(serde_json::to_value(ev).unwrap_or_default()),
    );
    body.insert("topic".to_string(), Variant::from(ev.event.clone()));
    body
}

impl OntologyTriggerNode {
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
                    "{NODE} : VALKEY_URL is not set in the runtime environment"
                ))
            })?;
        let client = redis::Client::open(url.as_str()).map_err(|e| {
            EdgelinkError::InvalidOperation(format!("{NODE} : invalid VALKEY_URL: {e}"))
        })?;
        Ok(Box::new(OntologyTriggerNode {
            base: base_node,
            trigger: cfg.trigger(),
            own_source: format!("flow:{}@", cfg.pnex_flow_id),
            config: cfg,
            client,
        }))
    }

    /// One subscription session: returns when the connection drops (the caller
    /// reconnects) or the node stops.
    async fn session(&self, stop: &CancellationToken) -> std::result::Result<(), String> {
        let channel = ontology_channel(self.config.pnex_org_id);
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
            let Ok(ev) = serde_json::from_str::<OntologyEvent>(&raw) else {
                continue;
            };
            if !self.trigger.matches(&ev, &self.own_source) {
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
impl FlowNodeBehavior for OntologyTriggerNode {
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

    #[test]
    fn message_carries_the_event_and_its_kind_as_topic() {
        let ev = OntologyEvent {
            event: "object".into(),
            action: "object.update".into(),
            object_id: "o1".into(),
            object_type: "person".into(),
            changed: vec!["city".into()],
            link_type: String::new(),
            other_id: String::new(),
            origin: String::new(),
            proposal_id: String::new(),
            source_ref: "manual:1".into(),
            ts_ms: 5,
        };
        let body = body_of(&ev);
        let payload = serde_json::to_value(&body["payload"]).unwrap();
        assert_eq!(payload["object_id"], "o1");
        assert_eq!(payload["changed"][0], "city");
        assert_eq!(serde_json::to_value(&body["topic"]).unwrap(), "object");
    }

    #[test]
    fn config_reads_the_projection() {
        let raw = serde_json::json!({
            "type": "pnex-ontology-trigger",
            "object_type": "person",
            "on": ["object", "proposal"],
            "skip_own": true,
            "pnex_org_id": 3,
            "pnex_flow_id": 9,
        });
        let cfg = NodeConfig::deserialize(&raw).unwrap();
        assert_eq!(cfg.pnex_flow_id, 9);
        assert!(cfg.trigger().check_deployable().is_none());
    }
}
