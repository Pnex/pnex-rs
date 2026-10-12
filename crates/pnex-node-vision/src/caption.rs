//! `pnex-vision-caption` node (media-vision-studio.md D195) — describes a
//! camera frame with the organization's vision-language model, through the
//! backend (`/internal/flow/ai/caption`: the backend reads the frame from
//! the bus itself and holds the provider key; the runtime never sees it).
//!
//! - Input: any message whose payload carries `frame_key` (camera-source,
//!   vision-detect, vision-track port 0).
//! - At most one caption per source every `min_interval_secs`; frames
//!   older than [`STALE_FRAME_MS`] are skipped (gone from the bus anyway).
//! - `crop_best`: the best detection / track of the message is described
//!   instead of the whole frame (skipped when the message has none).
//! - Output: `{caption, caption_id, model, ts_ms, frame_key, label?,
//!   device_id | stream, ai_generated: true}`.
//! - Status (D103): last success or the backend's error code (no provider,
//!   daily cap reached, frame expired…), never a silent failure.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{json, Value};
use tokio_util::sync::CancellationToken;

use edgelink_core::runtime::context::*;
use edgelink_core::runtime::flow::*;
use edgelink_core::runtime::model::json::*;
use edgelink_core::runtime::model::*;
use edgelink_core::runtime::nodes::*;
use edgelink_core::{EdgelinkError, Result};
use edgelink_macro::*;

use pnex_core::vision::{NodeStatus, NodeStatusLevel};
use pnex_core::vision_ai::{CaptionRequest, CaptionResponse, VisionCaptionConfig};
use pnex_node_camera::status::{publish_status, STATUS_PERIOD};

use crate::track::{int_field, source_of};

const NODE: &str = "pnex-vision-caption";
/// Frames older than this are skipped (the bus keeps them 15 s).
const STALE_FRAME_MS: i64 = 10_000;

#[derive(Debug, Deserialize)]
struct CaptionNodeConfig {
    #[serde(flatten)]
    cfg: VisionCaptionConfig,
    #[serde(default)]
    pnex_org_id: i64,
    #[serde(default)]
    pnex_node_id: String,
}

/// Highest-score `{bbox, label}` among `detections` / `tracks`.
pub fn best_box(payload: &Value) -> Option<([f32; 4], String)> {
    ["detections", "tracks"]
        .iter()
        .filter_map(|k| payload.get(*k).and_then(Value::as_array))
        .flatten()
        .filter_map(|d| {
            let score = d.get("score").and_then(Value::as_f64).unwrap_or(0.0);
            let b: [f32; 4] = serde_json::from_value(d.get("bbox")?.clone()).ok()?;
            let label = d.get("label").and_then(Value::as_str).unwrap_or_default();
            Some((score, b, label.to_string()))
        })
        .max_by(|a, b| a.0.total_cmp(&b.0))
        .map(|(_, b, l)| (b, l))
}

/// POSTs `body` to an internal flow AI route; `Err` = `"<code>: <error>"`.
pub async fn post_ai<B: serde::Serialize, R: serde::de::DeserializeOwned>(
    http: &reqwest::Client,
    url: &str,
    token: &str,
    body: &B,
) -> std::result::Result<R, String> {
    let resp = http
        .post(url)
        .header("x-pnex-flow-token", token)
        .header(
            pnex_core::FLOW_WORKER_HEADER,
            pnex_core::flow_worker_fence().unwrap_or_default(),
        )
        .json(body)
        .send()
        .await
        .map_err(|e| format!("unreachable: {e}"))?;
    if resp.status().is_success() {
        return resp.json().await.map_err(|e| format!("unreadable: {e}"));
    }
    let status = resp.status();
    let v: Value = resp.json().await.unwrap_or(Value::Null);
    Err(format!(
        "{}: {}",
        v["code"].as_str().unwrap_or(status.as_str()),
        v["error"].as_str().unwrap_or_default()
    ))
}

#[derive(Default)]
struct Stats {
    received: AtomicU64,
    throttled: AtomicU64,
    stale: AtomicU64,
    failed: AtomicU64,
    emitted: AtomicU64,
}

#[flow_node("pnex-vision-caption", red_name = "pnex-vision-caption")]
struct VisionCaptionNode {
    base: BaseFlowNodeState,
    config: CaptionNodeConfig,
    http: reqwest::Client,
    url: String,
    token: String,
    /// Wall-clock ms of the last caption call per source.
    last: std::sync::Mutex<HashMap<String, i64>>,
    stats: Stats,
    /// Backend error of the last failed call, cleared by a success.
    last_error: std::sync::Mutex<Option<String>>,
}

impl VisionCaptionNode {
    fn build(
        _flow: &Flow,
        base_node: BaseFlowNodeState,
        config: &RedFlowNodeConfig,
        _options: Option<&config::Config>,
    ) -> Result<Box<dyn FlowNodeBehavior>> {
        let config = CaptionNodeConfig::deserialize(&config.rest)
            .map_err(|e| EdgelinkError::BadFlowsJson(format!("{NODE} : invalid config : {e}")))?;
        if let Some((code, message)) = config.cfg.check() {
            return Err(EdgelinkError::BadFlowsJson(format!("{NODE} [{code}] : {message}")).into());
        }
        if config.pnex_org_id <= 0 {
            return Err(EdgelinkError::BadFlowsJson(format!(
                "{NODE} : pnex_org_id missing from the artifact (redeploy the flow)"
            ))
            .into());
        }
        let env = |k: &str| {
            std::env::var(k).map_err(|_| {
                EdgelinkError::InvalidOperation(format!(
                    "{NODE} : {k} is not set in the runtime environment"
                ))
            })
        };
        let url = format!("{}/caption", env("PNEX_FLOW_AI_URL")?.trim_end_matches('/'));
        let token = env("PNEX_FLOW_WRITE_TOKEN")?;
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(120))
            .build()
            .map_err(|e| EdgelinkError::InvalidOperation(format!("{NODE} : http client: {e}")))?;
        Ok(Box::new(VisionCaptionNode {
            base: base_node,
            config,
            http,
            url,
            token,
            last: std::sync::Mutex::new(HashMap::new()),
            stats: Stats::default(),
            last_error: std::sync::Mutex::new(None),
        }))
    }

    fn status(&self) -> NodeStatus {
        let s = &self.stats;
        let base = match self.last_error.lock().expect("caption error").clone() {
            Some(e) => NodeStatus::new(NodeStatusLevel::Error, "ai-call-failed").detail(e),
            None if s.emitted.load(Ordering::Relaxed) == 0 => {
                NodeStatus::new(NodeStatusLevel::Warn, "vision-no-frames")
            }
            None => NodeStatus::new(NodeStatusLevel::Ok, "vision-running"),
        };
        [
            ("received", &s.received),
            ("throttled", &s.throttled),
            ("stale", &s.stale),
            ("failed", &s.failed),
            ("emitted", &s.emitted),
        ]
        .into_iter()
        .fold(base, |st, (k, v)| st.stat(k, v.load(Ordering::Relaxed)))
    }

    async fn on_msg(&self, payload: Value, cancel: CancellationToken) -> Result<()> {
        self.stats.received.fetch_add(1, Ordering::Relaxed);
        let (Some((field, id)), Some(key), Some(ts_ms)) = (
            source_of(&payload),
            payload.get("frame_key").and_then(Value::as_str),
            int_field(&payload, "ts_ms"),
        ) else {
            log::warn!(
                "{NODE} [{}] : message ignored — expected a frame reference (payload.frame_key)",
                self.name()
            );
            return Ok(());
        };
        let now = chrono::Utc::now().timestamp_millis();
        if now - ts_ms > STALE_FRAME_MS {
            self.stats.stale.fetch_add(1, Ordering::Relaxed);
            return Ok(());
        }
        let crop = if self.config.cfg.crop_best {
            match best_box(&payload) {
                Some(b) => Some(b),
                None => return Ok(()),
            }
        } else {
            None
        };
        {
            let mut last = self.last.lock().expect("caption last");
            let gap = (self.config.cfg.min_interval_secs * 1000.0) as i64;
            if last.get(&id).is_some_and(|t| now - t < gap) {
                self.stats.throttled.fetch_add(1, Ordering::Relaxed);
                return Ok(());
            }
            last.insert(id.clone(), now);
        }
        let req = CaptionRequest {
            org_id: self.config.pnex_org_id,
            frame_key: key.to_string(),
            prompt: self.config.cfg.effective_prompt().to_string(),
            crop: crop.as_ref().map(|(b, _)| *b),
        };
        let res: std::result::Result<CaptionResponse, String> =
            post_ai(&self.http, &self.url, &self.token, &req).await;
        let res = match res {
            Ok(r) => r,
            Err(e) => {
                log::warn!("{NODE} [{}] : caption failed: {e}", self.name());
                self.stats.failed.fetch_add(1, Ordering::Relaxed);
                *self.last_error.lock().expect("caption error") = Some(e);
                publish_status(self, &self.config.pnex_node_id, &self.status());
                return Ok(());
            }
        };
        let had_error = self
            .last_error
            .lock()
            .expect("caption error")
            .take()
            .is_some();
        let mut out = json!({
            field: id,
            "ts_ms": ts_ms,
            "frame_key": key,
            "caption": res.caption,
            "caption_id": format!("cap-{id}-{ts_ms}"),
            "model": res.model,
            "took_ms": res.took_ms,
            "ai_generated": true,
        });
        if let Some((_, label)) = crop {
            out["label"] = Value::from(label);
        }
        let mut body = std::collections::BTreeMap::new();
        body.insert("payload".to_string(), Variant::from(out));
        body.insert("topic".to_string(), Variant::from(id));
        self.fan_out_one(
            Envelope {
                port: 0,
                msg: MsgHandle::with_properties(body),
            },
            cancel,
        )
        .await?;
        self.stats.emitted.fetch_add(1, Ordering::Relaxed);
        if had_error {
            publish_status(self, &self.config.pnex_node_id, &self.status());
        }
        Ok(())
    }
}

#[async_trait]
impl FlowNodeBehavior for VisionCaptionNode {
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
        while !stop_token.is_cancelled() {
            let cancel = stop_token.child_token();
            with_uow(
                self.as_ref(),
                cancel.child_token(),
                |node: &VisionCaptionNode, msg: MsgHandle| async move {
                    let payload = {
                        let m = msg.read().await;
                        m.get("payload")
                            .map(|v| serde_json::to_value(v).unwrap_or(Value::Null))
                            .unwrap_or(Value::Null)
                    };
                    node.on_msg(payload, cancel.child_token()).await
                },
            )
            .await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn best_box_over_detections_and_tracks() {
        let p = json!({
            "detections": [{"label": "dog", "score": 0.4, "bbox": [1, 2, 3, 4]}],
            "tracks": [{"label": "person", "score": 0.9, "bbox": [5, 6, 7, 8]}],
        });
        assert_eq!(
            best_box(&p),
            Some(([5.0, 6.0, 7.0, 8.0], "person".to_string()))
        );
        assert_eq!(best_box(&json!({"frame_key": "k"})), None);
    }
}
