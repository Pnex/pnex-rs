//! `pnex-vision-detect` node (camera-video.md D83) — runs a registry model
//! on `camera-source` frames and emits the detections.
//!
//! - The model (spec + ONNX bytes) is pulled from the backend internal
//!   routes (`/internal/flow/ml-model/{id}`) when the node starts, loaded
//!   once on the blocking pool; the flow keeps running while it loads
//!   (frames are skipped until it is ready).
//! - Frames come by reference (`payload.frame_key` → Valkey GET).
//! - Backpressure: per-camera `max_fps` sampling, and frames older than
//!   [`STALE_FRAME_MS`] are skipped (a slow CPU never builds a backlog).
//! - Output (port 0): `payload = {device_id, seq, ts_ms, width, height,
//!   frame_key, took_ms, count, labels, detections, best_below}` — only
//!   when something matched in `on_detection` mode (default), on every
//!   frame in `always`.
//! - Threshold: `min_score` when > 0, else the model threshold. Inference
//!   runs at [`DETECT_FLOOR`] and filters afterwards, so a lower `min_score`
//!   really lowers the bar and "seen but below the threshold" is counted.
//! - Annotation layer (D105, `record_layer`): the matched boxes of every
//!   analysed frame are buffered per camera and posted every
//!   [`LAYER_FLUSH`] to `/internal/flow/video-annotations` (OpenObserve
//!   `camera_detections`) — boxes only, drawn over the recordings in the
//!   Cameras page. A failed post is logged and dropped, never retried.
//! - Status (D103): model loading / ready / load failed, and a heartbeat
//!   with every drop reason counted (`stale`, `throttled`, `not_ready`,
//!   `no_frame`, `failed`, `below_threshold`) — never a silent drop.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use serde::Deserialize;
use tokio::sync::{Mutex, OnceCell};
use tokio_util::sync::CancellationToken;

use edgelink_core::runtime::context::*;
use edgelink_core::runtime::flow::*;
use edgelink_core::runtime::model::json::*;
use edgelink_core::runtime::model::*;
use edgelink_core::runtime::nodes::*;
use edgelink_core::{EdgelinkError, Result};
use edgelink_macro::*;

use pnex_core::camera::{layer_id_of, AnnotationBatch, FrameAnnotation};
use pnex_core::vision::{
    Detection, NodeStatus, NodeStatusLevel, VisionDetectConfig, VisionEmit, DETECT_FLOOR,
};
use pnex_node_camera::status::{publish_status, STATUS_PERIOD};
use pnex_vision::Detector;

const NODE: &str = "pnex-vision-detect";
const FLOW_TOKEN_HEADER: &str = "x-pnex-flow-token";
/// Frames older than this are skipped (inference slower than the camera).
const STALE_FRAME_MS: i64 = 3_000;
/// Annotation layer post period (D105).
const LAYER_FLUSH: Duration = Duration::from_secs(5);
/// Buffered annotations per camera that trigger an early post.
const LAYER_BATCH_MAX: usize = 200;

/// Anti-stripping anchor (see the runtime binary).
pub fn registered() {}

#[derive(Debug, Deserialize)]
struct DetectNodeConfig {
    model_id: String,
    #[serde(default)]
    labels: Vec<String>,
    #[serde(default)]
    min_score: f32,
    #[serde(default)]
    emit: String,
    #[serde(default = "d_fps")]
    max_fps: f64,
    /// Store the matched boxes as an annotation layer (D105).
    #[serde(default)]
    record_layer: bool,
    #[serde(default)]
    pnex_org_id: i64,
    #[serde(default)]
    pnex_flow_id: i64,
    /// Canvas id (status badge matching, D103).
    #[serde(default)]
    pnex_node_id: String,
}

/// Longest wait between two model load attempts.
const LOAD_BACKOFF_MAX_SECS: u64 = 30;

/// Counters behind the status heartbeat (D103) — one per drop reason.
#[derive(Default)]
struct DetectStats {
    received: AtomicU64,
    /// Model not loaded yet (or failed).
    not_ready: AtomicU64,
    /// Older than [`STALE_FRAME_MS`] when picked up.
    stale: AtomicU64,
    /// Skipped by the `max_fps` sampling.
    throttled: AtomicU64,
    /// Frame bytes expired or unreadable from Valkey.
    no_frame: AtomicU64,
    /// Decode or inference error.
    failed: AtomicU64,
    analysed: AtomicU64,
    /// Analysed frames whose best match stayed under the threshold.
    below_threshold: AtomicU64,
    emitted: AtomicU64,
    last_infer_ms: AtomicU64,
}

/// Model lifecycle as shown in the status.
#[derive(Clone)]
enum ModelState {
    Loading,
    Ready,
    Failed(String),
}

/// Score 0–1 → rounded percent.
fn percent_of(score: f32) -> u32 {
    (score.clamp(0.0, 1.0) * 100.0).round() as u32
}

/// One-line summary, best score first: `person 87%, dog 64%` (empty when
/// nothing matched).
fn summary_of(detections: &[Detection]) -> String {
    let mut sorted: Vec<&Detection> = detections.iter().collect();
    sorted.sort_by(|a, b| b.score.total_cmp(&a.score));
    sorted
        .iter()
        .map(|d| format!("{} {}%", d.label, percent_of(d.score)))
        .collect::<Vec<_>>()
        .join(", ")
}

fn d_fps() -> f64 {
    1.0
}

#[flow_node("pnex-vision-detect", red_name = "pnex-vision-detect")]
struct VisionDetectNode {
    base: BaseFlowNodeState,
    config: DetectNodeConfig,
    emit: VisionEmit,
    valkey: redis::aio::ConnectionManager,
    http: reqwest::Client,
    model_url: String,
    token: String,
    detector: OnceCell<Arc<Detector>>,
    last_accepted: Mutex<HashMap<String, i64>>,
    stats: DetectStats,
    model_state: std::sync::Mutex<ModelState>,
    /// `PNEX_FLOW_ANNOTATION_URL`, only read when `record_layer` is on.
    layer_url: Option<String>,
    /// Model name (layer display name fallback), set once loaded.
    model_name: std::sync::Mutex<String>,
    /// Annotations waiting for the next layer post, per camera.
    layer_buf: Mutex<HashMap<String, Vec<FrameAnnotation>>>,
}

impl VisionDetectNode {
    fn build(
        _flow: &Flow,
        base_node: BaseFlowNodeState,
        config: &RedFlowNodeConfig,
        _options: Option<&config::Config>,
    ) -> Result<Box<dyn FlowNodeBehavior>> {
        let cfg = DetectNodeConfig::deserialize(&config.rest)
            .map_err(|e| EdgelinkError::BadFlowsJson(format!("{NODE} : invalid config : {e}")))?;
        let emit = match cfg.emit.as_str() {
            "" | "on_detection" => VisionEmit::OnDetection,
            "always" => VisionEmit::Always,
            other => {
                return Err(EdgelinkError::BadFlowsJson(format!(
                    "{NODE} : unknown emit mode `{other}`"
                ))
                .into())
            }
        };
        let check = VisionDetectConfig {
            model_id: cfg.model_id.clone(),
            labels: cfg.labels.clone(),
            min_score: cfg.min_score,
            emit,
            max_fps: cfg.max_fps,
            record_layer: cfg.record_layer,
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
        let url = std::env::var("VALKEY_URL")
            .ok()
            .filter(|u| !u.trim().is_empty())
            .ok_or_else(|| {
                EdgelinkError::InvalidOperation(format!(
                    "{NODE} [camera_bus_unavailable] : VALKEY_URL is not set in the runtime environment"
                ))
            })?;
        let client = redis::Client::open(url.as_str()).map_err(|e| {
            EdgelinkError::InvalidOperation(format!("{NODE} : invalid VALKEY_URL: {e}"))
        })?;
        let valkey = redis::aio::ConnectionManager::new_lazy_with_config(
            client,
            redis::aio::ConnectionManagerConfig::new(),
        )
        .map_err(|e| EdgelinkError::InvalidOperation(format!("{NODE} : valkey client: {e}")))?;
        let model_url = std::env::var("PNEX_FLOW_MODEL_URL").map_err(|_| {
            EdgelinkError::InvalidOperation(format!(
                "{NODE} : PNEX_FLOW_MODEL_URL is not set in the runtime environment \
                 (runtime_token not configured on the server?)"
            ))
        })?;
        let token = std::env::var("PNEX_FLOW_WRITE_TOKEN").map_err(|_| {
            EdgelinkError::InvalidOperation(format!(
                "{NODE} : PNEX_FLOW_WRITE_TOKEN is not set in the runtime environment"
            ))
        })?;
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(120))
            .build()
            .map_err(|e| EdgelinkError::InvalidOperation(format!("{NODE} : http client: {e}")))?;
        let layer_url = if cfg.record_layer {
            Some(std::env::var("PNEX_FLOW_ANNOTATION_URL").map_err(|_| {
                EdgelinkError::InvalidOperation(format!(
                    "{NODE} : PNEX_FLOW_ANNOTATION_URL is not set in the runtime environment \
                     (server older than the annotation layers?)"
                ))
            })?)
        } else {
            None
        };
        Ok(Box::new(VisionDetectNode {
            base: base_node,
            config: cfg,
            emit,
            valkey,
            http,
            model_url,
            token,
            detector: OnceCell::new(),
            last_accepted: Mutex::new(HashMap::new()),
            stats: DetectStats::default(),
            model_state: std::sync::Mutex::new(ModelState::Loading),
            layer_url,
            model_name: std::sync::Mutex::new(String::new()),
            layer_buf: Mutex::new(HashMap::new()),
        }))
    }

    /// Pulls the model from the backend and loads it (blocking pool).
    async fn load_model(&self) -> std::result::Result<Detector, String> {
        let base = format!(
            "{}/{}",
            self.model_url.trim_end_matches('/'),
            self.config.model_id
        );
        let org = [("org_id", self.config.pnex_org_id.to_string())];
        let meta: serde_json::Value = self
            .http
            .get(&base)
            .query(&org)
            .header(FLOW_TOKEN_HEADER, &self.token)
            .send()
            .await
            .and_then(|r| r.error_for_status())
            .map_err(|e| format!("model spec fetch failed: {e}"))?
            .json()
            .await
            .map_err(|e| format!("model spec unreadable: {e}"))?;
        let model: pnex_core::vision::MlModel = serde_json::from_value(meta["model"].clone())
            .map_err(|e| format!("model spec unreadable: {e}"))?;
        let bytes = self
            .http
            .get(format!("{base}/content"))
            .query(&org)
            .header(FLOW_TOKEN_HEADER, &self.token)
            .send()
            .await
            .and_then(|r| r.error_for_status())
            .map_err(|e| format!("model download failed: {e}"))?
            .bytes()
            .await
            .map_err(|e| format!("model download failed: {e}"))?;
        *self.model_name.lock().expect("model name") = model.name.clone();
        let spec = model.spec;
        tokio::task::spawn_blocking(move || Detector::load(&bytes, spec))
            .await
            .map_err(|e| e.to_string())?
            .map_err(|e| e.to_string())
    }

    /// Effective threshold: the node override, else the model's.
    fn threshold(&self, det: &Detector) -> f32 {
        if self.config.min_score > 0.0 {
            self.config.min_score
        } else {
            det.spec().score_threshold
        }
    }

    fn label_ok(&self, d: &Detection) -> bool {
        self.config.labels.is_empty() || self.config.labels.iter().any(|l| l == &d.label)
    }

    fn set_state(&self, state: ModelState) {
        *self.model_state.lock().expect("model state") = state;
        publish_status(self, &self.config.pnex_node_id, &self.status());
    }

    /// Current status: model state first, then counters.
    fn status(&self) -> NodeStatus {
        let s = &self.stats;
        let base = match self.model_state.lock().expect("model state").clone() {
            ModelState::Loading => NodeStatus::new(NodeStatusLevel::Warn, "vision-model-loading"),
            ModelState::Failed(e) => {
                NodeStatus::new(NodeStatusLevel::Error, "vision-model-load-failed").detail(e)
            }
            ModelState::Ready if s.received.load(Ordering::Relaxed) == 0 => {
                NodeStatus::new(NodeStatusLevel::Warn, "vision-no-frames")
            }
            ModelState::Ready => NodeStatus::new(NodeStatusLevel::Ok, "vision-running"),
        };
        [
            ("received", &s.received),
            ("not_ready", &s.not_ready),
            ("stale", &s.stale),
            ("throttled", &s.throttled),
            ("no_frame", &s.no_frame),
            ("failed", &s.failed),
            ("analysed", &s.analysed),
            ("below_threshold", &s.below_threshold),
            ("emitted", &s.emitted),
            ("last_infer_ms", &s.last_infer_ms),
        ]
        .into_iter()
        .fold(base, |st, (k, v)| st.stat(k, v.load(Ordering::Relaxed)))
    }

    /// Layer display name: the node name, else the model name, else the id.
    fn layer_name(&self) -> String {
        let name = self.name().trim().to_string();
        if !name.is_empty() {
            return name;
        }
        let model = self.model_name.lock().expect("model name").clone();
        if model.is_empty() {
            self.config.pnex_node_id.clone()
        } else {
            model
        }
    }

    /// Posts the buffered annotations (one batch per camera).
    async fn flush_layer(&self) {
        let Some(url) = self.layer_url.as_deref() else {
            return;
        };
        let pending: Vec<(String, Vec<FrameAnnotation>)> = self
            .layer_buf
            .lock()
            .await
            .drain()
            .filter(|(_, a)| !a.is_empty())
            .collect();
        for (device_id, annotations) in pending {
            let n = annotations.len();
            let batch = AnnotationBatch {
                org_id: self.config.pnex_org_id,
                device_id,
                layer_id: layer_id_of(self.config.pnex_flow_id, &self.config.pnex_node_id),
                layer: self.layer_name(),
                flow_id: (self.config.pnex_flow_id > 0).then_some(self.config.pnex_flow_id),
                annotations,
            };
            let res = self
                .http
                .post(url)
                .header(FLOW_TOKEN_HEADER, &self.token)
                .header(
                    pnex_core::FLOW_WORKER_HEADER,
                    pnex_core::flow_worker_fence().unwrap_or_default(),
                )
                .json(&batch)
                .send()
                .await;
            match res {
                Ok(r) if r.status().is_success() => {}
                Ok(r) => log::warn!(
                    "{NODE} [{}] : annotation layer refused ({}) — {n} frames dropped",
                    self.name(),
                    r.status()
                ),
                Err(e) => log::warn!(
                    "{NODE} [{}] : annotation layer post failed: {e} — {n} frames dropped",
                    self.name()
                ),
            }
        }
    }

    async fn on_frame(&self, msg: MsgHandle, cancel: CancellationToken) -> Result<()> {
        self.stats.received.fetch_add(1, Ordering::Relaxed);
        let Some(det) = self.detector.get().cloned() else {
            self.stats.not_ready.fetch_add(1, Ordering::Relaxed);
            return Ok(());
        };
        let payload = {
            let m = msg.read().await;
            m.get("payload")
                .map(|v| serde_json::to_value(v).unwrap_or(serde_json::Value::Null))
                .unwrap_or(serde_json::Value::Null)
        };
        // A device camera, or the video track of a media stream (D175).
        let source_field = if payload.get("device_id").is_some() {
            "device_id"
        } else {
            "stream"
        };
        let (Some(device_id), Some(key), Some(ts_ms)) = (
            payload
                .get(source_field)
                .and_then(|v| v.as_str())
                .map(str::to_string),
            payload
                .get("frame_key")
                .and_then(|v| v.as_str())
                .map(str::to_string),
            payload.get("ts_ms").and_then(|v| v.as_i64()),
        ) else {
            log::warn!(
                "{NODE} [{}] : message ignored — expected a camera-source frame",
                self.name()
            );
            return Ok(());
        };
        if !pnex_core::camera::frame_key_in_org(self.config.pnex_org_id, &key) {
            log::warn!(
                "{NODE} [{}] : message ignored — frame key outside the organization",
                self.name()
            );
            return Ok(());
        }
        if chrono::Utc::now().timestamp_millis() - ts_ms > STALE_FRAME_MS {
            self.stats.stale.fetch_add(1, Ordering::Relaxed);
            return Ok(());
        }
        {
            let mut last = self.last_accepted.lock().await;
            let rate_key = format!("{source_field}:{device_id}");
            let prev = last.get(&rate_key).copied();
            if self.config.max_fps > 0.0 {
                let gap = (1000.0 / self.config.max_fps) as i64;
                if prev.is_some_and(|p| ts_ms - p < gap - gap / 10) {
                    self.stats.throttled.fetch_add(1, Ordering::Relaxed);
                    return Ok(());
                }
            }
            last.insert(rate_key, ts_ms);
        }
        let mut conn = self.valkey.clone();
        let jpeg: Option<Vec<u8>> = match redis::AsyncCommands::get(&mut conn, &key).await {
            Ok(v) => v,
            Err(e) => {
                log::warn!("{NODE} [{}] : frame fetch failed: {e}", self.name());
                self.stats.no_frame.fetch_add(1, Ordering::Relaxed);
                return Ok(());
            }
        };
        let Some(jpeg) = jpeg else {
            self.stats.no_frame.fetch_add(1, Ordering::Relaxed);
            return Ok(());
        };
        let threshold = self.threshold(&det);
        let res =
            match tokio::task::spawn_blocking(move || det.detect_with_floor(&jpeg, DETECT_FLOOR))
                .await
            {
                Ok(Ok(r)) => r,
                Ok(Err(e)) => {
                    log::warn!("{NODE} [{}] : inference failed: {e}", self.name());
                    self.stats.failed.fetch_add(1, Ordering::Relaxed);
                    return Ok(());
                }
                Err(e) => {
                    log::warn!("{NODE} [{}] : inference task failed: {e}", self.name());
                    self.stats.failed.fetch_add(1, Ordering::Relaxed);
                    return Ok(());
                }
            };
        self.stats.analysed.fetch_add(1, Ordering::Relaxed);
        self.stats
            .last_infer_ms
            .store(res.took_ms, Ordering::Relaxed);
        let (detections, below): (Vec<Detection>, Vec<Detection>) = res
            .detections
            .into_iter()
            .filter(|d| self.label_ok(d))
            .partition(|d| d.score >= threshold);
        if detections.is_empty() && !below.is_empty() {
            self.stats.below_threshold.fetch_add(1, Ordering::Relaxed);
        }
        // Annotation layers are stored per device camera only.
        if self.layer_url.is_some() && source_field == "device_id" && !detections.is_empty() {
            let full = {
                let mut buf = self.layer_buf.lock().await;
                let anns = buf.entry(device_id.clone()).or_default();
                anns.push(FrameAnnotation {
                    layer_id: String::new(),
                    ts_ms,
                    width: res.width,
                    height: res.height,
                    detections: detections.clone(),
                });
                anns.len() >= LAYER_BATCH_MAX
            };
            if full {
                self.flush_layer().await;
            }
        }
        if detections.is_empty() && self.emit == VisionEmit::OnDetection {
            return Ok(());
        }
        // Best candidate under the threshold: tells why nothing matched.
        let best_below = below
            .iter()
            .max_by(|a, b| a.score.total_cmp(&b.score))
            .map(|d| serde_json::json!({ "label": d.label, "score": d.score }));
        let mut labels: Vec<String> = detections.iter().map(|d| d.label.clone()).collect();
        labels.sort();
        labels.dedup();
        // Human-friendly view next to the raw scores (0–1): `percent` per
        // detection and a one-line `summary` ("person 87%, dog 64%").
        let detections_out: Vec<serde_json::Value> = detections
            .iter()
            .map(|d| {
                let mut v = serde_json::to_value(d).unwrap_or(serde_json::Value::Null);
                v["percent"] = serde_json::Value::from(percent_of(d.score));
                v
            })
            .collect();
        let out = serde_json::json!({
            "summary": summary_of(&detections),
            source_field: device_id,
            "seq": payload.get("seq").cloned().unwrap_or(serde_json::Value::Null),
            "ts_ms": ts_ms,
            "width": res.width,
            "height": res.height,
            "frame_key": key,
            "took_ms": res.took_ms,
            "count": detections.len(),
            "labels": labels,
            "detections": detections_out,
            "threshold": threshold,
            "best_below": best_below,
        });
        let mut body = std::collections::BTreeMap::new();
        body.insert("payload".to_string(), Variant::from(out));
        body.insert("topic".to_string(), Variant::from(device_id));
        self.fan_out_one(
            Envelope {
                port: 0,
                msg: MsgHandle::with_properties(body),
            },
            cancel,
        )
        .await?;
        self.stats.emitted.fetch_add(1, Ordering::Relaxed);
        Ok(())
    }
}

#[async_trait]
impl FlowNodeBehavior for VisionDetectNode {
    fn get_base(&self) -> &BaseFlowNodeState {
        &self.base
    }

    async fn run(self: Arc<Self>, stop_token: CancellationToken) {
        // Model load in the background: the node drains its inbox (frames
        // counted as `not_ready`) until the detector is ready.
        {
            let node = self.clone();
            let stop = stop_token.clone();
            tokio::spawn(async move {
                node.set_state(ModelState::Loading);
                let mut backoff = 2u64;
                while !stop.is_cancelled() {
                    match node.load_model().await {
                        Ok(d) => {
                            let _ = node.detector.set(Arc::new(d));
                            log::info!(
                                "{NODE} [{}] : model {} ready",
                                node.name(),
                                node.config.model_id
                            );
                            node.set_state(ModelState::Ready);
                            break;
                        }
                        Err(e) => {
                            log::warn!("{NODE} [{}] : {e} — retry in {backoff} s", node.name());
                            node.set_state(ModelState::Failed(e));
                            tokio::select! {
                                _ = stop.cancelled() => break,
                                _ = tokio::time::sleep(Duration::from_secs(backoff)) => {}
                            }
                            backoff = (backoff * 2).min(LOAD_BACKOFF_MAX_SECS);
                        }
                    }
                }
            });
        }
        // Status heartbeat.
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
        // Annotation layer flusher (D105).
        if self.layer_url.is_some() {
            let node = self.clone();
            let stop = stop_token.clone();
            tokio::spawn(async move {
                loop {
                    tokio::select! {
                        _ = stop.cancelled() => break,
                        _ = tokio::time::sleep(LAYER_FLUSH) => {}
                    }
                    node.flush_layer().await;
                }
            });
        }
        while !stop_token.is_cancelled() {
            let cancel = stop_token.child_token();
            with_uow(
                self.as_ref(),
                cancel.child_token(),
                |node: &VisionDetectNode, msg: MsgHandle| async move {
                    node.on_frame(msg, cancel.child_token()).await
                },
            )
            .await;
        }
        // Stop / redeploy: keep what was analysed.
        self.flush_layer().await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn det(label: &str, score: f32) -> Detection {
        Detection {
            label: label.into(),
            class_id: 0,
            score,
            bbox: [0.0; 4],
        }
    }

    #[test]
    fn summary_and_percent() {
        assert_eq!(percent_of(0.874), 87);
        assert_eq!(percent_of(1.5), 100);
        assert_eq!(
            summary_of(&[det("dog", 0.64), det("person", 0.87)]),
            "person 87%, dog 64%"
        );
        assert_eq!(summary_of(&[]), "");
    }
}
