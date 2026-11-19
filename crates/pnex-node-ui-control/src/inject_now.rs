//! `pnex-inject-now` — companion of an Inject node (run_now.rs in
//! pnex-core): emits the Inject's message when a person runs the flow now.
//!
//! The backend checks the request, then publishes a `RunNowEvent` on
//! `pnex:run-now:v1:{org}`. This node subscribes to the org channel and
//! emits one message per event naming its flow and Inject node. No replay:
//! a click is an event, not a state.

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

use pnex_core::flow::{run_now_channel, RunNowEvent};

const NODE: &str = "pnex-inject-now";
const BACKOFF_MAX_SECS: u64 = 30;

#[derive(Debug, Deserialize)]
struct NodeConfig {
    // No `serde(flatten)`: it breaks numbers under `arbitrary_precision`.
    /// Canvas id of the Inject node it stands for.
    #[serde(default)]
    inject_of: String,
    /// The Inject's payload; null = the time of the click, as an Inject.
    #[serde(default)]
    payload: serde_json::Value,
    #[serde(default)]
    topic: Option<String>,
    #[serde(default)]
    pnex_flow_id: i64,
    #[serde(default)]
    pnex_org_id: i64,
}

#[flow_node("pnex-inject-now", red_name = "pnex-inject-now")]
struct InjectNowNode {
    base: BaseFlowNodeState,
    config: NodeConfig,
    client: redis::Client,
}

/// Message body of one run-now event: the Inject's own message.
fn body_of(cfg: &NodeConfig, ev: &RunNowEvent) -> BTreeMap<String, Variant> {
    let mut body = BTreeMap::new();
    let payload = match &cfg.payload {
        serde_json::Value::Null => serde_json::json!(ev.ts_ms),
        p => p.clone(),
    };
    body.insert("payload".to_string(), Variant::from(payload));
    if let Some(t) = &cfg.topic {
        body.insert("topic".to_string(), Variant::from(t.clone()));
    }
    body
}

impl InjectNowNode {
    fn build(
        _flow: &Flow,
        base_node: BaseFlowNodeState,
        config: &RedFlowNodeConfig,
        _options: Option<&config::Config>,
    ) -> Result<Box<dyn FlowNodeBehavior>> {
        let cfg = NodeConfig::deserialize(&config.rest)
            .map_err(|e| EdgelinkError::BadFlowsJson(format!("{NODE} : invalid config : {e}")))?;
        if cfg.pnex_org_id <= 0 || cfg.pnex_flow_id <= 0 || cfg.inject_of.is_empty() {
            return Err(EdgelinkError::BadFlowsJson(format!(
                "{NODE} : org, flow or inject node missing from the artifact (redeploy the flow)"
            ))
            .into());
        }
        let url = std::env::var("VALKEY_URL")
            .ok()
            .filter(|u| !u.trim().is_empty())
            .ok_or_else(|| {
                EdgelinkError::InvalidOperation(format!(
                    "{NODE} [run_now_bus_unavailable] : VALKEY_URL is not set in the runtime environment"
                ))
            })?;
        let client = redis::Client::open(url.as_str()).map_err(|e| {
            EdgelinkError::InvalidOperation(format!("{NODE} : invalid VALKEY_URL: {e}"))
        })?;
        Ok(Box::new(InjectNowNode {
            base: base_node,
            config: cfg,
            client,
        }))
    }

    /// One subscription session: returns when the connection drops (the
    /// caller reconnects) or the node stops.
    async fn session(&self, stop: &CancellationToken) -> std::result::Result<(), String> {
        let channel = run_now_channel(self.config.pnex_org_id);
        let mut pubsub = self
            .client
            .get_async_pubsub()
            .await
            .map_err(|e| format!("valkey connect failed: {e}"))?;
        pubsub
            .subscribe(&channel)
            .await
            .map_err(|e| format!("valkey subscribe failed: {e}"))?;
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
            let Ok(ev) = serde_json::from_str::<RunNowEvent>(&raw) else {
                continue;
            };
            if !ev.matches(self.config.pnex_flow_id, &self.config.inject_of) {
                continue;
            }
            let envelope = Envelope {
                port: 0,
                msg: MsgHandle::with_properties(body_of(&self.config, &ev)),
            };
            if let Err(e) = self.fan_out_one(envelope, stop.child_token()).await {
                log::warn!("{NODE} [{}] : fan-out failed: {e}", self.name());
            }
        }
    }
}

#[async_trait]
impl FlowNodeBehavior for InjectNowNode {
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

    fn cfg(raw: serde_json::Value) -> NodeConfig {
        NodeConfig::deserialize(&raw).unwrap()
    }

    #[test]
    fn the_message_is_the_injects_own() {
        let c = cfg(serde_json::json!({
            "type": NODE, "inject_of": "tick", "payload": {"a": 1}, "topic": "chimie",
            "pnex_flow_id": 54, "pnex_org_id": 3,
        }));
        let ev = RunNowEvent {
            flow_id: 54,
            node_id: "tick".into(),
            by: None,
            ts_ms: 9,
        };
        let body = body_of(&c, &ev);
        assert_eq!(serde_json::to_value(&body["payload"]).unwrap()["a"], 1);
        assert_eq!(serde_json::to_value(&body["topic"]).unwrap(), "chimie");
    }

    #[test]
    fn a_timestamp_inject_sends_the_time_of_the_click() {
        let c = cfg(serde_json::json!({
            "type": NODE, "inject_of": "tick", "pnex_flow_id": 54, "pnex_org_id": 3,
        }));
        let ev = RunNowEvent {
            flow_id: 54,
            node_id: "tick".into(),
            by: None,
            ts_ms: 1_790_000_000_000,
        };
        let body = body_of(&c, &ev);
        assert_eq!(
            serde_json::to_value(&body["payload"]).unwrap(),
            1_790_000_000_000i64
        );
        assert!(!body.contains_key("topic"));
    }
}
