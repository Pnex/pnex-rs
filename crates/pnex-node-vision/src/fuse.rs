//! `pnex-media-fuse` node (media-vision-studio.md D196) — every window,
//! sends what the flow received (transcripts, detection / tracking / zone
//! events, captions) to the organization's LLM through the backend
//! (`/internal/flow/ai/fuse`) and emits a short description whose every
//! sentence cites its inputs (D184 provenance; unsourced sentences are
//! dropped by the backend).
//!
//! Inputs are normalized here into [`FuseItem`]s (kind, id, time, source,
//! one line of text); an unknown payload is dropped with a warning. At
//! most [`FUSE_ITEMS_MAX`] items per window: the oldest are dropped first.
//! Output, once per non-empty window: `{t0_ms, t1_ms, text, sentences:
//! [{text, sources}], items, rejected, dropped, model, ai_generated}`.

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
use pnex_core::vision_ai::{
    clip, FuseItem, FuseKind, FuseRequest, FuseResponse, MediaFuseConfig, FUSE_ITEMS_MAX,
    FUSE_ITEM_TEXT_MAX,
};
use pnex_node_camera::status::publish_status;

use crate::caption::post_ai;
use crate::track::{int_field, source_of};

const NODE: &str = "pnex-media-fuse";

#[derive(Debug, Deserialize)]
struct FuseNodeConfig {
    #[serde(flatten)]
    cfg: MediaFuseConfig,
    #[serde(default)]
    pnex_org_id: i64,
    #[serde(default)]
    pnex_node_id: String,
}

fn str_of<'a>(p: &'a Value, k: &str) -> Option<&'a str> {
    p.get(k).and_then(Value::as_str)
}

/// One input message → one fusion item (`None` = not a known shape).
pub fn item_of(payload: &Value, now_ms: i64) -> Option<FuseItem> {
    let ts_ms = int_field(payload, "ts_ms").unwrap_or(now_ms);
    let source = source_of(payload)
        .map(|(_, id)| id)
        .or_else(|| str_of(payload, "stream").map(str::to_string))
        .unwrap_or_default();
    let label = str_of(payload, "label").unwrap_or("object");
    let track = payload.get("track_id").and_then(Value::as_u64);
    let (kind, id, text) = if let Some(caption) = str_of(payload, "caption") {
        let id = str_of(payload, "caption_id")
            .map(str::to_string)
            .unwrap_or_else(|| format!("cap-{source}-{ts_ms}"));
        let text = match str_of(payload, "label") {
            Some(l) => format!("({l}) {caption}"),
            None => caption.to_string(),
        };
        (FuseKind::Caption, id, text)
    } else if let (Some(text), Some(seg)) = (str_of(payload, "text"), payload.get("segment_id")) {
        let seg = seg
            .as_str()
            .map(str::to_string)
            .unwrap_or_else(|| seg.to_string());
        let text = match str_of(payload, "speaker") {
            Some(s) => format!("{s}: {text}"),
            None => text.to_string(),
        };
        (FuseKind::Transcript, format!("seg-{seg}"), text)
    } else if let (Some(event), Some(zone)) = (str_of(payload, "event"), str_of(payload, "zone")) {
        let count = payload.get("count").and_then(Value::as_u64).unwrap_or(0);
        let t = track.map(|t| format!(" #{t}")).unwrap_or_default();
        let verb = if event == "enter" { "entered" } else { "left" };
        let text = format!("{label}{t} {verb} zone {zone} ({count} now inside)");
        (
            FuseKind::Detection,
            format!("zone-{zone}-{event}-{}-{ts_ms}", track.unwrap_or(0)),
            text,
        )
    } else if let (Some(event @ ("appear" | "disappear")), Some(t)) =
        (str_of(payload, "event"), track)
    {
        let dur = payload
            .get("duration_s")
            .and_then(Value::as_f64)
            .unwrap_or(0.0);
        let text = if event == "appear" {
            format!("{label} #{t} appeared")
        } else {
            format!("{label} #{t} disappeared after {dur:.0} s in view")
        };
        (FuseKind::Detection, format!("trk-{t}-{event}"), text)
    } else if let Some(summary) = str_of(payload, "summary").filter(|s| !s.is_empty()) {
        (
            FuseKind::Detection,
            format!("det-{source}-{ts_ms}"),
            format!("detected: {summary}"),
        )
    } else {
        let text = str_of(payload, "text")?;
        (
            FuseKind::Transcript,
            format!("txt-{source}-{ts_ms}"),
            text.to_string(),
        )
    };
    Some(FuseItem {
        id,
        kind,
        ts_ms,
        source,
        text: clip(&text, FUSE_ITEM_TEXT_MAX),
    })
}

/// Window buffer: pushes with unique ids and the size cap.
#[derive(Default)]
pub struct Window {
    pub items: Vec<FuseItem>,
    pub dropped: usize,
}

impl Window {
    pub fn push(&mut self, mut item: FuseItem) {
        let base = item.id.clone();
        let mut n = 1;
        while self.items.iter().any(|i| i.id == item.id) {
            n += 1;
            item.id = format!("{base}#{n}");
        }
        if self.items.len() >= FUSE_ITEMS_MAX {
            self.items.remove(0);
            self.dropped += 1;
        }
        self.items.push(item);
    }
}

#[derive(Default)]
struct Stats {
    received: AtomicU64,
    ignored: AtomicU64,
    windows: AtomicU64,
    failed: AtomicU64,
}

#[flow_node("pnex-media-fuse", red_name = "pnex-media-fuse")]
struct MediaFuseNode {
    base: BaseFlowNodeState,
    config: FuseNodeConfig,
    http: reqwest::Client,
    url: String,
    token: String,
    window: std::sync::Mutex<Window>,
    stats: Stats,
    last_error: std::sync::Mutex<Option<String>>,
}

impl MediaFuseNode {
    fn build(
        _flow: &Flow,
        base_node: BaseFlowNodeState,
        config: &RedFlowNodeConfig,
        _options: Option<&config::Config>,
    ) -> Result<Box<dyn FlowNodeBehavior>> {
        let config = FuseNodeConfig::deserialize(&config.rest)
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
        let url = format!("{}/fuse", env("PNEX_FLOW_AI_URL")?.trim_end_matches('/'));
        let token = env("PNEX_FLOW_WRITE_TOKEN")?;
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(120))
            .build()
            .map_err(|e| EdgelinkError::InvalidOperation(format!("{NODE} : http client: {e}")))?;
        Ok(Box::new(MediaFuseNode {
            base: base_node,
            config,
            http,
            url,
            token,
            window: std::sync::Mutex::new(Window::default()),
            stats: Stats::default(),
            last_error: std::sync::Mutex::new(None),
        }))
    }

    fn status(&self) -> NodeStatus {
        let s = &self.stats;
        let base = match self.last_error.lock().expect("fuse error").clone() {
            Some(e) => NodeStatus::new(NodeStatusLevel::Error, "ai-call-failed").detail(e),
            None => NodeStatus::new(NodeStatusLevel::Ok, "vision-running"),
        };
        [
            ("received", &s.received),
            ("ignored", &s.ignored),
            ("windows", &s.windows),
            ("failed", &s.failed),
        ]
        .into_iter()
        .fold(base, |st, (k, v)| st.stat(k, v.load(Ordering::Relaxed)))
    }

    /// Sends the current window, if any, and emits the description.
    async fn flush(&self, cancel: CancellationToken) -> Result<()> {
        let window = std::mem::take(&mut *self.window.lock().expect("fuse window"));
        if window.items.is_empty() {
            return Ok(());
        }
        let t0_ms = window.items.iter().map(|i| i.ts_ms).min().unwrap_or(0);
        let t1_ms = window.items.iter().map(|i| i.ts_ms).max().unwrap_or(0);
        let req = FuseRequest {
            org_id: self.config.pnex_org_id,
            t0_ms,
            t1_ms,
            language: self.config.cfg.language.clone(),
            items: window.items,
        };
        let res: std::result::Result<FuseResponse, String> =
            post_ai(&self.http, &self.url, &self.token, &req).await;
        let res = match res {
            Ok(r) => {
                *self.last_error.lock().expect("fuse error") = None;
                r
            }
            Err(e) => {
                log::warn!("{NODE} [{}] : fusion failed: {e}", self.name());
                self.stats.failed.fetch_add(1, Ordering::Relaxed);
                *self.last_error.lock().expect("fuse error") = Some(e);
                publish_status(self, &self.config.pnex_node_id, &self.status());
                return Ok(());
            }
        };
        self.stats.windows.fetch_add(1, Ordering::Relaxed);
        publish_status(self, &self.config.pnex_node_id, &self.status());
        if res.sentences.is_empty() {
            return Ok(());
        }
        let text = res
            .sentences
            .iter()
            .map(|s| s.text.as_str())
            .collect::<Vec<_>>()
            .join(" ");
        let out = json!({
            "t0_ms": t0_ms,
            "t1_ms": t1_ms,
            "text": text,
            "sentences": res.sentences,
            "items": req.items.len(),
            "rejected": res.rejected,
            "dropped": window.dropped,
            "model": res.model,
            "ai_generated": true,
        });
        let mut body = std::collections::BTreeMap::new();
        body.insert("payload".to_string(), Variant::from(out));
        body.insert("topic".to_string(), Variant::from("media-fuse"));
        self.fan_out_one(
            Envelope {
                port: 0,
                msg: MsgHandle::with_properties(body),
            },
            cancel,
        )
        .await
    }
}

#[async_trait]
impl FlowNodeBehavior for MediaFuseNode {
    fn get_base(&self) -> &BaseFlowNodeState {
        &self.base
    }

    async fn run(self: Arc<Self>, stop_token: CancellationToken) {
        {
            let node = self.clone();
            let stop = stop_token.clone();
            let period = Duration::from_secs_f64(node.config.cfg.window_secs);
            tokio::spawn(async move {
                loop {
                    tokio::select! {
                        _ = stop.cancelled() => break,
                        _ = tokio::time::sleep(period) => {}
                    }
                    if let Err(e) = node.flush(stop.child_token()).await {
                        log::warn!("{NODE} [{}] : output failed: {e}", node.name());
                    }
                }
            });
        }
        while !stop_token.is_cancelled() {
            let cancel = stop_token.child_token();
            with_uow(
                self.as_ref(),
                cancel.child_token(),
                |node: &MediaFuseNode, msg: MsgHandle| async move {
                    node.stats.received.fetch_add(1, Ordering::Relaxed);
                    let payload = {
                        let m = msg.read().await;
                        m.get("payload")
                            .map(|v| serde_json::to_value(v).unwrap_or(Value::Null))
                            .unwrap_or(Value::Null)
                    };
                    match item_of(&payload, chrono::Utc::now().timestamp_millis()) {
                        Some(item) => node.window.lock().expect("fuse window").push(item),
                        None => {
                            node.stats.ignored.fetch_add(1, Ordering::Relaxed);
                            log::warn!(
                                "{NODE} [{}] : message ignored — expected a transcript, a vision event or a caption",
                                node.name()
                            );
                        }
                    }
                    Ok(())
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
    fn items_of_each_input_shape() {
        let cap = item_of(
            &json!({"device_id": "cam", "ts_ms": 5, "caption": "a red forklift", "caption_id": "c1"}),
            0,
        )
        .unwrap();
        assert_eq!(
            (cap.kind, cap.id.as_str(), cap.source.as_str()),
            (FuseKind::Caption, "c1", "cam")
        );
        let seg = item_of(
            &json!({"stream": "radio", "segment_id": "s9", "text": "hello", "speaker": "S1"}),
            7,
        )
        .unwrap();
        assert_eq!(
            (seg.kind, seg.text.as_str(), seg.ts_ms),
            (FuseKind::Transcript, "S1: hello", 7)
        );
        let zone = item_of(
            &json!({"event": "enter", "zone": "dock", "label": "person", "track_id": 3, "count": 2, "ts_ms": 1}),
            0,
        )
        .unwrap();
        assert_eq!(zone.text, "person #3 entered zone dock (2 now inside)");
        let gone = item_of(
            &json!({"event": "disappear", "label": "car", "track_id": 4, "duration_s": 12.4}),
            0,
        )
        .unwrap();
        assert_eq!(gone.text, "car #4 disappeared after 12 s in view");
        let det = item_of(
            &json!({"device_id": "cam", "summary": "dog 80%", "ts_ms": 2}),
            0,
        )
        .unwrap();
        assert_eq!(det.text, "detected: dog 80%");
        assert!(item_of(&json!({"foo": 1}), 0).is_none());
        assert!(item_of(&json!(42), 0).is_none());
    }

    #[test]
    fn window_ids_are_unique_and_bounded() {
        let item = |id: &str| FuseItem {
            id: id.into(),
            kind: FuseKind::Transcript,
            ts_ms: 0,
            source: String::new(),
            text: "x".into(),
        };
        let mut w = Window::default();
        w.push(item("a"));
        w.push(item("a"));
        assert_eq!(w.items[1].id, "a#2");
        for i in 0..FUSE_ITEMS_MAX {
            w.push(item(&format!("n{i}")));
        }
        assert_eq!(w.items.len(), FUSE_ITEMS_MAX);
        assert_eq!(w.dropped, 2);
    }
}
