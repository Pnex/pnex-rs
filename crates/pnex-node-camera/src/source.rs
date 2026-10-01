//! `pnex-camera-source` — event-driven frame source (see crate docs).

use std::sync::atomic::{AtomicI64, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
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

use pnex_core::camera::BusFrameMeta;
use pnex_core::vision::{NodeStatus, NodeStatusLevel};

use crate::status::{publish_status, STATUS_PERIOD};

const NODE: &str = "pnex-camera-source";
const BACKOFF_MAX_SECS: u64 = 30;

#[derive(Debug, Deserialize)]
struct SourceNodeConfig {
    device_id: String,
    #[serde(default)]
    max_fps: f64,
    /// Stamped by the deploy projection.
    #[serde(default)]
    pnex_org_id: i64,
    /// Canvas id (status badge matching, D103).
    #[serde(default)]
    pnex_node_id: String,
}

/// Counters behind the status heartbeat (D103).
#[derive(Default)]
struct SourceStats {
    received: AtomicU64,
    /// Skipped by the `max_fps` sampling.
    throttled: AtomicU64,
    emitted: AtomicU64,
    last_frame_ms: AtomicI64,
    /// Last bus failure (cleared on a successful subscription).
    bus_error: Mutex<Option<String>>,
}

#[flow_node("pnex-camera-source", red_name = "pnex-camera-source")]
struct CameraSourceNode {
    base: BaseFlowNodeState,
    config: SourceNodeConfig,
    client: redis::Client,
    stats: SourceStats,
}

impl CameraSourceNode {
    fn build(
        _flow: &Flow,
        base_node: BaseFlowNodeState,
        config: &RedFlowNodeConfig,
        _options: Option<&config::Config>,
    ) -> Result<Box<dyn FlowNodeBehavior>> {
        let cfg = SourceNodeConfig::deserialize(&config.rest)
            .map_err(|e| EdgelinkError::BadFlowsJson(format!("{NODE} : invalid config : {e}")))?;
        let check = pnex_core::CameraSourceConfig {
            device_id: cfg.device_id.clone(),
            max_fps: cfg.max_fps,
        };
        if let Some((code, message)) = check.check() {
            return Err(EdgelinkError::BadFlowsJson(format!("{NODE} [{code}] : {message}")).into());
        }
        if cfg.pnex_org_id <= 0 {
            return Err(EdgelinkError::BadFlowsJson(format!(
                "{NODE} : pnex_org_id missing from the artifact (redeploy the flow)"
            ))
            .into());
        }
        let client = crate::valkey_client(NODE)?;
        Ok(Box::new(CameraSourceNode {
            base: base_node,
            config: cfg,
            client,
            stats: SourceStats::default(),
        }))
    }

    /// One subscription session: returns when the connection drops (the
    /// caller reconnects) or the node stops.
    async fn session(&self, stop: &CancellationToken) -> std::result::Result<(), String> {
        let channel =
            pnex_core::camera::bus_channel(self.config.pnex_org_id, &self.config.device_id);
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
        let mut stream = pubsub.on_message();
        let mut last_ms: Option<i64> = None;
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
            let Ok(meta) = serde_json::from_str::<BusFrameMeta>(&raw) else {
                continue;
            };
            self.stats.received.fetch_add(1, Ordering::Relaxed);
            self.stats
                .last_frame_ms
                .store(meta.ts_ms, Ordering::Relaxed);
            if !crate::sample_ok(self.config.max_fps, last_ms, meta.ts_ms) {
                self.stats.throttled.fetch_add(1, Ordering::Relaxed);
                continue;
            }
            last_ms = Some(meta.ts_ms);
            let payload = serde_json::json!({
                "device_id": self.config.device_id,
                "seq": meta.seq,
                "ts_ms": meta.ts_ms,
                "width": meta.width,
                "height": meta.height,
                "size": meta.size,
                "frame_key": meta.key,
            });
            let mut body = std::collections::BTreeMap::new();
            body.insert("payload".to_string(), crate::variant_of(payload));
            body.insert(
                "topic".to_string(),
                Variant::from(self.config.device_id.clone()),
            );
            let envelope = Envelope {
                port: 0,
                msg: MsgHandle::with_properties(body),
            };
            match self.fan_out_one(envelope, stop.child_token()).await {
                Ok(()) => {
                    self.stats.emitted.fetch_add(1, Ordering::Relaxed);
                }
                Err(e) => log::warn!("{NODE} [{}] : fan-out failed: {e}", self.name()),
            }
        }
    }
}

impl CameraSourceNode {
    /// Current status from the counters: bus down, no frame for a whole
    /// period (camera off, asleep or disconnected), else streaming.
    fn status(&self) -> NodeStatus {
        let s = &self.stats;
        let received = s.received.load(Ordering::Relaxed);
        let last = s.last_frame_ms.load(Ordering::Relaxed);
        let idle_ms = if last > 0 {
            chrono::Utc::now().timestamp_millis() - last
        } else {
            i64::MAX
        };
        let base = if let Some(err) = s.bus_error.lock().expect("stats").clone() {
            NodeStatus::new(NodeStatusLevel::Error, "camera-bus-unavailable").detail(err)
        } else if idle_ms > STATUS_PERIOD.as_millis() as i64 {
            NodeStatus::new(NodeStatusLevel::Warn, "camera-no-frames")
                .detail(self.config.device_id.clone())
        } else {
            NodeStatus::new(NodeStatusLevel::Ok, "camera-streaming")
        };
        base.stat("received", received)
            .stat("throttled", s.throttled.load(Ordering::Relaxed))
            .stat("emitted", s.emitted.load(Ordering::Relaxed))
    }
}

#[async_trait]
impl FlowNodeBehavior for CameraSourceNode {
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
                    publish_status(node.as_ref(), &node.config.pnex_node_id, &node.status());
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
                    publish_status(self.as_ref(), &self.config.pnex_node_id, &self.status());
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
