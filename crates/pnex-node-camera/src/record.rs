//! `pnex-video-record` — MJPEG-AVI segment recorder (see crate docs).
//!
//! One buffer per camera (`payload.device_id` of the incoming
//! `camera-source` message, or `payload.stream` for the video track of a
//! media stream, D175). A segment is flushed when its duration reaches
//! `segment_secs`, its payload reaches `max_segment_mb`, the frame size
//! changes, no frame arrived for `gap_secs`, or the node stops. Upload is
//! lenient: a failed segment is logged and dropped, the flow never stops.
//! Output: one message per stored segment (`payload` = the segment DTO).

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use serde::Deserialize;
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;

use edgelink_core::runtime::context::*;
use edgelink_core::runtime::flow::*;
use edgelink_core::runtime::model::json::*;
use edgelink_core::runtime::model::*;
use edgelink_core::runtime::nodes::*;
use edgelink_core::{EdgelinkError, Result};
use edgelink_macro::*;

use pnex_core::avi::AviWriter;

const NODE: &str = "pnex-video-record";
const FLOW_TOKEN_HEADER: &str = "x-pnex-flow-token";
const UPLOAD_TIMEOUT_SECS: u64 = 120;

#[derive(Debug, Deserialize)]
struct RecordNodeConfig {
    #[serde(default = "d_segment_secs")]
    segment_secs: u32,
    #[serde(default = "d_segment_mb")]
    max_segment_mb: u32,
    #[serde(default = "d_gap_secs")]
    gap_secs: u32,
    #[serde(default)]
    max_fps: f64,
    #[serde(default = "d_retention")]
    retention_days: u32,
    #[serde(default)]
    stream: String,
    #[serde(default)]
    pnex_node_id: String,
    #[serde(default)]
    pnex_flow_id: i64,
    #[serde(default)]
    pnex_org_id: i64,
}

fn d_segment_secs() -> u32 {
    pnex_core::VideoRecordConfig::default().segment_secs
}
fn d_segment_mb() -> u32 {
    pnex_core::VideoRecordConfig::default().max_segment_mb
}
fn d_gap_secs() -> u32 {
    pnex_core::VideoRecordConfig::default().gap_secs
}
fn d_retention() -> u32 {
    pnex_core::VideoRecordConfig::default().retention_days
}

/// Where a frame comes from (D175): a device camera or a media stream.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) enum Source {
    Device(String),
    Stream(String),
}

impl Source {
    /// Source of a `camera-source` payload.
    pub(crate) fn of(payload: &serde_json::Value) -> Option<Self> {
        let get = |k: &str| payload.get(k).and_then(|v| v.as_str()).map(str::to_string);
        get("device_id")
            .map(Self::Device)
            .or_else(|| get("stream").map(Self::Stream))
    }

    pub(crate) fn slug(&self) -> &str {
        match self {
            Self::Device(s) | Self::Stream(s) => s,
        }
    }

    /// Query parameter naming the source on the segment upload.
    fn query_key(&self) -> &'static str {
        match self {
            Self::Device(_) => "device_id",
            Self::Stream(_) => "media_stream",
        }
    }
}

/// Segment being accumulated for one camera.
struct Segment {
    writer: AviWriter,
    first_ms: i64,
    last_ms: i64,
    /// Wall clock of the last accepted frame (gap detection).
    last_arrival: std::time::Instant,
}

/// A finished segment ready for upload.
struct Finished {
    source: Source,
    first_ms: i64,
    last_ms: i64,
    width: u16,
    height: u16,
    frames: usize,
    avi: Vec<u8>,
}

impl Segment {
    fn finish(self, source: Source) -> Finished {
        let frames = self.writer.frame_count();
        let (width, height) = self.writer.dims();
        let span_s = (self.last_ms - self.first_ms) as f64 / 1000.0;
        // Real-time playback: frames spread over the observed span (N frames
        // cover N-1 intervals).
        let fps = if frames > 1 && span_s > 0.0 {
            (frames - 1) as f64 / span_s
        } else {
            1.0
        };
        Finished {
            source,
            first_ms: self.first_ms,
            last_ms: self.last_ms,
            width,
            height,
            frames,
            avi: self.writer.finish(fps),
        }
    }
}

#[flow_node("pnex-video-record", red_name = "pnex-video-record")]
struct VideoRecordNode {
    base: BaseFlowNodeState,
    config: RecordNodeConfig,
    valkey: redis::aio::ConnectionManager,
    http: reqwest::Client,
    url: String,
    token: String,
    segments: Mutex<HashMap<Source, Segment>>,
    last_accepted: Mutex<HashMap<Source, i64>>,
}

impl VideoRecordNode {
    fn build(
        _flow: &Flow,
        base_node: BaseFlowNodeState,
        config: &RedFlowNodeConfig,
        _options: Option<&config::Config>,
    ) -> Result<Box<dyn FlowNodeBehavior>> {
        let cfg = RecordNodeConfig::deserialize(&config.rest)
            .map_err(|e| EdgelinkError::BadFlowsJson(format!("{NODE} : invalid config : {e}")))?;
        let check = pnex_core::VideoRecordConfig {
            segment_secs: cfg.segment_secs,
            max_segment_mb: cfg.max_segment_mb,
            gap_secs: cfg.gap_secs,
            max_fps: cfg.max_fps,
            retention_days: cfg.retention_days,
            stream: cfg.stream.clone(),
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
        let valkey = redis::aio::ConnectionManager::new_lazy_with_config(
            client,
            redis::aio::ConnectionManagerConfig::new(),
        )
        .map_err(|e| EdgelinkError::InvalidOperation(format!("{NODE} : valkey client: {e}")))?;
        // Injected by the supervisor next to the device-write coordinates
        // (same host and service token) — absent = fail loud at deploy.
        let url = std::env::var("PNEX_FLOW_VIDEO_URL").map_err(|_| {
            EdgelinkError::InvalidOperation(format!(
                "{NODE} : PNEX_FLOW_VIDEO_URL is not set in the runtime environment \
                 (runtime_token not configured on the server?)"
            ))
        })?;
        let token = std::env::var("PNEX_FLOW_WRITE_TOKEN").map_err(|_| {
            EdgelinkError::InvalidOperation(format!(
                "{NODE} : PNEX_FLOW_WRITE_TOKEN is not set in the runtime environment"
            ))
        })?;
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(UPLOAD_TIMEOUT_SECS))
            .build()
            .map_err(|e| EdgelinkError::InvalidOperation(format!("{NODE} : http client: {e}")))?;
        Ok(Box::new(VideoRecordNode {
            base: base_node,
            config: cfg,
            valkey,
            http,
            url,
            token,
            segments: Mutex::new(HashMap::new()),
            last_accepted: Mutex::new(HashMap::new()),
        }))
    }

    /// Handles one `camera-source` message.
    async fn on_frame(&self, msg: MsgHandle, cancel: CancellationToken) -> Result<()> {
        let payload = {
            let m = msg.read().await;
            match m.get("payload") {
                Some(v) => serde_json::to_value(v).unwrap_or(serde_json::Value::Null),
                None => serde_json::Value::Null,
            }
        };
        let (Some(source), Some(key), Some(ts_ms)) = (
            Source::of(&payload),
            payload.get("frame_key").and_then(|v| v.as_str()),
            payload.get("ts_ms").and_then(|v| v.as_i64()),
        ) else {
            log::warn!(
                "{NODE} [{}] : message ignored — expected a camera-source frame payload",
                self.name()
            );
            return Ok(());
        };
        if !pnex_core::camera::frame_key_in_org(self.config.pnex_org_id, key) {
            log::warn!(
                "{NODE} [{}] : message ignored — frame key outside the organization",
                self.name()
            );
            return Ok(());
        }
        let width = payload.get("width").and_then(|v| v.as_u64()).unwrap_or(0) as u16;
        let height = payload.get("height").and_then(|v| v.as_u64()).unwrap_or(0) as u16;
        {
            let mut last = self.last_accepted.lock().await;
            if !crate::sample_ok(self.config.max_fps, last.get(&source).copied(), ts_ms) {
                return Ok(());
            }
            last.insert(source.clone(), ts_ms);
        }
        let mut conn = self.valkey.clone();
        let jpeg: Option<Vec<u8>> = match redis::AsyncCommands::get(&mut conn, key).await {
            Ok(v) => v,
            Err(e) => {
                log::warn!("{NODE} [{}] : frame fetch failed: {e}", self.name());
                return Ok(());
            }
        };
        // Expired (slow branch) or evicted: skip the frame, keep recording.
        let Some(jpeg) = jpeg else {
            return Ok(());
        };

        let mut ready = Vec::new();
        {
            let mut segs = self.segments.lock().await;
            // Resolution change: close the current segment first.
            if let Some(seg) = segs.get(&source) {
                if seg.writer.dims() != (width, height) {
                    let seg = segs.remove(&source).expect("present");
                    ready.push(seg.finish(source.clone()));
                }
            }
            let seg = segs.entry(source.clone()).or_insert_with(|| Segment {
                writer: AviWriter::new(width, height),
                first_ms: ts_ms,
                last_ms: ts_ms,
                last_arrival: std::time::Instant::now(),
            });
            seg.writer.push(jpeg);
            seg.last_ms = ts_ms;
            seg.last_arrival = std::time::Instant::now();
            let full_time =
                seg.last_ms - seg.first_ms >= i64::from(self.config.segment_secs) * 1000;
            let full_size =
                seg.writer.payload_bytes() >= self.config.max_segment_mb as usize * 1024 * 1024;
            if full_time || full_size {
                let seg = segs.remove(&source).expect("present");
                ready.push(seg.finish(source));
            }
        }
        for f in ready {
            self.upload(f, cancel.clone()).await;
        }
        Ok(())
    }

    /// Flushes segments idle for more than `gap_secs` (or all of them).
    async fn flush_idle(&self, all: bool, cancel: CancellationToken) {
        let gap = Duration::from_secs(u64::from(self.config.gap_secs));
        let ready: Vec<Finished> = {
            let mut segs = self.segments.lock().await;
            let idle: Vec<Source> = segs
                .iter()
                .filter(|(_, s)| all || s.last_arrival.elapsed() >= gap)
                .map(|(k, _)| k.clone())
                .collect();
            idle.into_iter()
                .filter_map(|k| segs.remove(&k).map(|s| s.finish(k)))
                .collect()
        };
        for f in ready {
            self.upload(f, cancel.clone()).await;
        }
    }

    async fn upload(&self, f: Finished, cancel: CancellationToken) {
        if f.frames == 0 {
            return;
        }
        let stream = if self.config.stream.trim().is_empty() {
            f.source.slug().to_string()
        } else {
            self.config.stream.trim().to_string()
        };
        let mut query: Vec<(&str, String)> = vec![
            ("org_id", self.config.pnex_org_id.to_string()),
            (f.source.query_key(), f.source.slug().to_string()),
            ("node_id", self.config.pnex_node_id.clone()),
            ("stream", stream),
            ("started_ms", f.first_ms.to_string()),
            ("ended_ms", f.last_ms.to_string()),
            ("frames", f.frames.to_string()),
            ("width", f.width.to_string()),
            ("height", f.height.to_string()),
            ("retention_days", self.config.retention_days.to_string()),
        ];
        if self.config.pnex_flow_id > 0 {
            query.push(("flow_id", self.config.pnex_flow_id.to_string()));
        }
        let size = f.avi.len();
        let res = self
            .http
            .post(&self.url)
            .header(FLOW_TOKEN_HEADER, &self.token)
            .header(
                pnex_core::FLOW_WORKER_HEADER,
                pnex_core::flow_worker_fence().unwrap_or_default(),
            )
            .header("content-type", "video/x-msvideo")
            .query(&query)
            .body(f.avi)
            .send()
            .await;
        let dto = match res {
            Ok(r) if r.status().is_success() => r.json::<serde_json::Value>().await.ok(),
            Ok(r) => {
                log::warn!(
                    "{NODE} [{}] : segment upload refused ({}) — {} frames dropped",
                    self.name(),
                    r.status(),
                    f.frames
                );
                return;
            }
            Err(e) => {
                log::warn!(
                    "{NODE} [{}] : segment upload failed: {e} — {} frames dropped",
                    self.name(),
                    f.frames
                );
                return;
            }
        };
        log::info!(
            "{NODE} [{}] : segment stored ({} frames, {size} bytes, {})",
            self.name(),
            f.frames,
            f.source.slug()
        );
        let payload = dto.unwrap_or_else(|| {
            serde_json::json!({
                f.source.query_key(): f.source.slug(),
                "frame_count": f.frames,
                "size_bytes": size,
            })
        });
        let mut body = std::collections::BTreeMap::new();
        body.insert("payload".to_string(), crate::variant_of(payload));
        body.insert(
            "topic".to_string(),
            Variant::from(f.source.slug().to_string()),
        );
        let envelope = Envelope {
            port: 0,
            msg: MsgHandle::with_properties(body),
        };
        if let Err(e) = self.fan_out_one(envelope, cancel).await {
            log::warn!("{NODE} [{}] : fan-out failed: {e}", self.name());
        }
    }
}

#[async_trait]
impl FlowNodeBehavior for VideoRecordNode {
    fn get_base(&self) -> &BaseFlowNodeState {
        &self.base
    }

    async fn run(self: Arc<Self>, stop_token: CancellationToken) {
        // Gap watcher: closes segments of cameras that stopped sending.
        let watcher = {
            let node = self.clone();
            let stop = stop_token.clone();
            tokio::spawn(async move {
                let mut tick = tokio::time::interval(Duration::from_secs(1));
                loop {
                    tokio::select! {
                        _ = stop.cancelled() => break,
                        _ = tick.tick() => node.flush_idle(false, stop.child_token()).await,
                    }
                }
            })
        };
        while !stop_token.is_cancelled() {
            let cancel = stop_token.child_token();
            with_uow(
                self.as_ref(),
                cancel.child_token(),
                |node: &VideoRecordNode, msg: MsgHandle| async move {
                    node.on_frame(msg, cancel.child_token()).await
                },
            )
            .await;
        }
        let _ = watcher.await;
        // Stop / redeploy: keep what was recorded (best effort, fresh token
        // — the stop token is already cancelled).
        self.flush_idle(true, CancellationToken::new()).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn source_of_payload() {
        let dev = Source::of(&serde_json::json!({"device_id": "cam", "frame_key": "k"}));
        assert_eq!(dev, Some(Source::Device("cam".into())));
        let st = Source::of(&serde_json::json!({"stream": "lobby"})).unwrap();
        assert_eq!(st, Source::Stream("lobby".into()));
        assert_eq!((st.query_key(), st.slug()), ("media_stream", "lobby"));
        assert_eq!(Source::of(&serde_json::json!({"x": 1})), None);
    }
}
