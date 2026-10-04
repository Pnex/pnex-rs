//! `pnex-control-source` — event-driven source of org controls (D127,
//! `docs/architecture/surfaces-controls.md`).
//!
//! A surface (dashboard card, annotation) writes a control through the
//! backend, which stores the `ControlEvent` under `pnex:ctl:v1:{org}:{id}`
//! and publishes it on `pnex:ctl:v1:{org}`. This node subscribes to the org
//! channel, keeps the events of its controls and emits one message per
//! write on the port of the control (`payload` = value, `topic` = control
//! key, `msg.control` = `{id, key, by, via, ts_ms}`).
//!
//! With `emit_on_start`, the stored values are replayed after each
//! (re)subscription, so the outputs fed by the flow recover their commanded
//! state after an engine restart, a redeploy or a Valkey reconnect.
//!
//! The node never writes a device itself: `device-write` downstream does,
//! under the "one write source per output pin" rule (D128).

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicI64, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use futures_util::StreamExt;
use serde::Deserialize;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use edgelink_core::runtime::context::*;
use edgelink_core::runtime::debug_channel::DebugMessage;
use edgelink_core::runtime::flow::*;
use edgelink_core::runtime::model::json::*;
use edgelink_core::runtime::model::*;
use edgelink_core::runtime::nodes::*;
use edgelink_core::{EdgelinkError, Result};
use edgelink_macro::*;

use pnex_core::ui_control::{
    control_channel, control_value_key, ControlEvent, ControlSourceConfig,
};
use pnex_core::vision::{NodeStatus, NodeStatusLevel, NODE_STATUS_FORMAT};

const NODE: &str = "pnex-control-source";
const BACKOFF_MAX_SECS: u64 = 30;
/// Heartbeat period of the node status (D103 school).
const STATUS_PERIOD: Duration = Duration::from_secs(10);
/// Bound of the start replay MGET.
const REPLAY_TIMEOUT: Duration = Duration::from_secs(2);

/// Anti-stripping anchor: the runtime binary calls it so the linker keeps
/// the `inventory` submission of the node.
pub fn registered() {}

#[derive(Debug, Deserialize)]
struct NodeConfig {
    #[serde(default)]
    controls: Vec<Uuid>,
    #[serde(default)]
    emit_on_start: bool,
    /// Stamped by the deploy projection.
    #[serde(default)]
    pnex_org_id: i64,
    /// Canvas id (status badge matching).
    #[serde(default)]
    pnex_node_id: String,
}

#[derive(Default)]
struct Stats {
    /// Writes of this node's controls seen on the bus (replays included).
    received: AtomicU64,
    emitted: AtomicU64,
    last_ms: AtomicI64,
    bus_error: Mutex<Option<String>>,
}

#[flow_node("pnex-control-source", red_name = "pnex-control-source")]
struct ControlSourceNode {
    base: BaseFlowNodeState,
    config: NodeConfig,
    /// Control id → output port.
    ports: BTreeMap<Uuid, usize>,
    client: redis::Client,
    stats: Stats,
}

/// Output port of an event, `None` when the control is not listened here.
fn port_of(ports: &BTreeMap<Uuid, usize>, ev: &ControlEvent) -> Option<usize> {
    ports.get(&ev.control_id).copied()
}

/// Message body of one control event.
fn body_of(ev: &ControlEvent) -> BTreeMap<String, Variant> {
    let mut body = BTreeMap::new();
    body.insert(
        "payload".to_string(),
        Variant::from(serde_json::json!(ev.value.v)),
    );
    body.insert("topic".to_string(), Variant::from(ev.key.clone()));
    body.insert(
        "control".to_string(),
        Variant::from(serde_json::json!({
            "id": ev.control_id.to_string(),
            "key": ev.key,
            "by": ev.value.by,
            "via": ev.value.via,
            "ts_ms": ev.value.ts_ms,
        })),
    );
    body
}

impl ControlSourceNode {
    fn build(
        _flow: &Flow,
        base_node: BaseFlowNodeState,
        config: &RedFlowNodeConfig,
        _options: Option<&config::Config>,
    ) -> Result<Box<dyn FlowNodeBehavior>> {
        let cfg = NodeConfig::deserialize(&config.rest)
            .map_err(|e| EdgelinkError::BadFlowsJson(format!("{NODE} : invalid config : {e}")))?;
        let check = ControlSourceConfig {
            controls: cfg.controls.clone(),
            emit_on_start: cfg.emit_on_start,
        };
        if let Some((code, message)) = check.check_deployable() {
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
                    "{NODE} [control_bus_unavailable] : VALKEY_URL is not set in the runtime \
                     environment (org controls live in Valkey)"
                ))
            })?;
        let client = redis::Client::open(url.as_str()).map_err(|e| {
            EdgelinkError::InvalidOperation(format!("{NODE} : invalid VALKEY_URL: {e}"))
        })?;
        let ports = cfg
            .controls
            .iter()
            .enumerate()
            .map(|(i, id)| (*id, i))
            .collect();
        Ok(Box::new(ControlSourceNode {
            base: base_node,
            config: cfg,
            ports,
            client,
            stats: Stats::default(),
        }))
    }

    async fn emit(&self, ev: &ControlEvent, stop: &CancellationToken) {
        let Some(port) = port_of(&self.ports, ev) else {
            return;
        };
        self.stats.received.fetch_add(1, Ordering::Relaxed);
        self.stats.last_ms.store(ev.value.ts_ms, Ordering::Relaxed);
        let envelope = Envelope {
            port,
            msg: MsgHandle::with_properties(body_of(ev)),
        };
        match self.fan_out_one(envelope, stop.child_token()).await {
            Ok(()) => {
                self.stats.emitted.fetch_add(1, Ordering::Relaxed);
            }
            Err(e) => log::warn!("{NODE} [{}] : fan-out failed: {e}", self.name()),
        }
    }

    /// Stored events of the listened controls, in port order (never
    /// written = skipped).
    async fn stored_events(&self) -> std::result::Result<Vec<ControlEvent>, String> {
        let keys: Vec<String> = self
            .config
            .controls
            .iter()
            .map(|id| control_value_key(self.config.pnex_org_id, *id))
            .collect();
        let client = self.client.clone();
        let op = async move {
            let mut conn = client.get_multiplexed_async_connection().await?;
            let raw: Vec<Option<String>> = redis::AsyncCommands::mget(&mut conn, &keys).await?;
            Ok::<_, redis::RedisError>(raw)
        };
        let raw = tokio::time::timeout(REPLAY_TIMEOUT, op)
            .await
            .map_err(|_| "valkey replay timed out".to_string())?
            .map_err(|e| format!("valkey replay failed: {e}"))?;
        Ok(raw
            .into_iter()
            .flatten()
            .filter_map(|r| serde_json::from_str::<ControlEvent>(&r).ok())
            .collect())
    }

    /// One subscription session: returns when the connection drops (the
    /// caller reconnects) or the node stops.
    async fn session(&self, stop: &CancellationToken) -> std::result::Result<(), String> {
        let channel = control_channel(self.config.pnex_org_id);
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
        *self.stats.bus_error.lock().expect("stats") = None;
        // Replay AFTER subscribing: a write landing in between is seen twice
        // (harmless, the last value wins) rather than lost.
        if self.config.emit_on_start {
            for ev in self.stored_events().await? {
                self.emit(&ev, stop).await;
            }
        }
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
            let Ok(ev) = serde_json::from_str::<ControlEvent>(&raw) else {
                continue;
            };
            self.emit(&ev, stop).await;
        }
    }

    fn status(&self) -> NodeStatus {
        let s = &self.stats;
        let base = match s.bus_error.lock().expect("stats").clone() {
            Some(err) => {
                NodeStatus::new(NodeStatusLevel::Error, "control-bus-unavailable").detail(err)
            }
            None => NodeStatus::new(NodeStatusLevel::Ok, "control-listening"),
        };
        base.stat("commands", s.received.load(Ordering::Relaxed))
            .stat("emitted", s.emitted.load(Ordering::Relaxed))
    }

    /// Publishes the status on the engine debug channel (D103 school).
    fn publish_status(&self) {
        let Some(engine) = self.engine() else {
            return;
        };
        let msg = serde_json::to_value(self.status()).unwrap_or(serde_json::Value::Null);
        let name = self.name();
        engine.debug_channel().send(DebugMessage {
            id: self.config.pnex_node_id.clone(),
            name: if name.is_empty() {
                None
            } else {
                Some(name.to_string())
            },
            msg,
            property: Some("payload".into()),
            format: Some(NODE_STATUS_FORMAT.into()),
            path: self
                .flow()
                .map(|f| f.get_path())
                .unwrap_or_else(|| "global".to_string()),
            topic: None,
            timestamp: Some(chrono::Utc::now().timestamp_millis()),
            msgid: None,
        });
    }
}

#[async_trait]
impl FlowNodeBehavior for ControlSourceNode {
    fn get_base(&self) -> &BaseFlowNodeState {
        &self.base
    }

    async fn run(self: Arc<Self>, stop_token: CancellationToken) {
        {
            let node = self.clone();
            let stop = stop_token.clone();
            tokio::spawn(async move {
                loop {
                    tokio::select! {
                        _ = stop.cancelled() => break,
                        _ = tokio::time::sleep(STATUS_PERIOD) => {}
                    }
                    node.publish_status();
                }
            });
        }
        let mut backoff = 1u64;
        while !stop_token.is_cancelled() {
            match self.session(&stop_token).await {
                Ok(()) => break,
                Err(e) => {
                    log::warn!("{NODE} [{}] : {e} — retry in {backoff} s", self.name());
                    *self.stats.bus_error.lock().expect("stats") = Some(e.clone());
                    self.publish_status();
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
    use pnex_core::ui_control::ControlValue;

    fn event(id: u128, v: f64) -> ControlEvent {
        ControlEvent {
            control_id: Uuid::from_u128(id),
            key: "light.room".into(),
            value: ControlValue {
                v,
                ts_ms: 1_790_000_000_000,
                by: Some("alice".into()),
                via: Some("dashboard:x".into()),
            },
        }
    }

    #[test]
    fn ports_follow_config_order_and_ignore_other_controls() {
        let ports: BTreeMap<Uuid, usize> = [(Uuid::from_u128(2), 0), (Uuid::from_u128(1), 1)]
            .into_iter()
            .collect();
        assert_eq!(port_of(&ports, &event(1, 1.0)), Some(1));
        assert_eq!(port_of(&ports, &event(2, 1.0)), Some(0));
        assert_eq!(port_of(&ports, &event(3, 1.0)), None);
    }

    #[test]
    fn message_carries_value_topic_and_origin() {
        let body = body_of(&event(1, 42.5));
        let payload = serde_json::to_value(&body["payload"]).unwrap();
        assert_eq!(payload.as_f64(), Some(42.5));
        assert_eq!(serde_json::to_value(&body["topic"]).unwrap(), "light.room");
        let control = serde_json::to_value(&body["control"]).unwrap();
        assert_eq!(control["id"], Uuid::from_u128(1).to_string());
        assert_eq!(control["by"], "alice");
        assert_eq!(control["via"], "dashboard:x");
    }
}
