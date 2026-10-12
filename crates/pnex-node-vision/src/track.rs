//! `pnex-vision-track` node (media-vision-studio.md D192) — follows the
//! `vision-detect` objects from frame to frame and gives each one a stable
//! `track_id`, per camera or stream.
//!
//! Algorithm (ByteTrack, MIT — reimplemented, no dependency):
//! - tracks are predicted forward with their box-centre velocity;
//! - stage 1 matches the high-score detections (`>= high_score`) to every
//!   track, stage 2 matches the low-score ones to the tracks left over, so
//!   a weak detection keeps an object alive without ever opening a track;
//! - matching is per label, by IoU (`>= min_iou`);
//! - a new track is tentative until `min_hits` matches (then `appear`), a
//!   tentative track that misses a frame is dropped silently, a confirmed
//!   one unseen for `lost_after_secs` is dropped with `disappear`.
//!
//! Ports: 0 = current confirmed tracks of the source (every analysed frame,
//! and when tracks expire without a new frame, so `vision-zone` sees the
//! exits); 1 = one message per `appear` / `disappear` event.

use std::collections::HashMap;
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

use pnex_core::vision::{Detection, VisionTrackConfig};

const NODE: &str = "pnex-vision-track";
/// Period of the expiry sweep for sources that stopped sending frames.
const SWEEP: Duration = Duration::from_secs(1);

/// One followed object.
#[derive(Debug, Clone)]
pub struct Track {
    pub id: u64,
    pub label: String,
    /// `[x, y, w, h]` in source pixels (last match).
    pub bbox: [f32; 4],
    pub score: f32,
    /// Box-centre velocity, px/ms.
    vel: [f32; 2],
    hits: u32,
    pub confirmed: bool,
    pub first_ts_ms: i64,
    pub last_ts_ms: i64,
    score_sum: f64,
}

impl Track {
    fn conf_avg(&self) -> f64 {
        self.score_sum / f64::from(self.hits.max(1))
    }

    /// Box shifted by the velocity up to `ts_ms`.
    fn predicted(&self, ts_ms: i64) -> [f32; 4] {
        let dt = (ts_ms - self.last_ts_ms).max(0) as f32;
        let [x, y, w, h] = self.bbox;
        [x + self.vel[0] * dt, y + self.vel[1] * dt, w, h]
    }

    pub fn to_json(&self) -> Value {
        json!({
            "track_id": self.id,
            "label": self.label,
            "score": self.score,
            "bbox": self.bbox,
            "first_ts_ms": self.first_ts_ms,
            "last_ts_ms": self.last_ts_ms,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrackEventKind {
    Appear,
    Disappear,
}

#[derive(Debug, Clone)]
pub struct TrackEvent {
    pub kind: TrackEventKind,
    pub track: Track,
}

impl TrackEvent {
    pub fn to_json(&self) -> Value {
        let t = &self.track;
        json!({
            "event": match self.kind {
                TrackEventKind::Appear => "appear",
                TrackEventKind::Disappear => "disappear",
            },
            "track_id": t.id,
            "label": t.label,
            "t0_ms": t.first_ts_ms,
            "t1_ms": t.last_ts_ms,
            "duration_s": (t.last_ts_ms - t.first_ts_ms) as f64 / 1000.0,
            "conf_avg": (t.conf_avg() * 1000.0).round() / 1000.0,
            "bbox": t.bbox,
        })
    }
}

fn iou(a: &[f32; 4], b: &[f32; 4]) -> f32 {
    let x1 = a[0].max(b[0]);
    let y1 = a[1].max(b[1]);
    let x2 = (a[0] + a[2]).min(b[0] + b[2]);
    let y2 = (a[1] + a[3]).min(b[1] + b[3]);
    let inter = (x2 - x1).max(0.0) * (y2 - y1).max(0.0);
    let union = a[2] * a[3] + b[2] * b[3] - inter;
    if union <= 0.0 {
        0.0
    } else {
        inter / union
    }
}

/// Tracks of one source.
#[derive(Debug, Default)]
pub struct Tracker {
    pub tracks: Vec<Track>,
}

impl Tracker {
    /// Drops the confirmed tracks unseen since `now_ms - lost`.
    pub fn expire(&mut self, cfg: &VisionTrackConfig, now_ms: i64) -> Vec<TrackEvent> {
        let lost_ms = (cfg.lost_after_secs * 1000.0) as i64;
        let mut events = Vec::new();
        self.tracks.retain(|t| {
            let alive = now_ms - t.last_ts_ms <= lost_ms;
            if !alive && t.confirmed {
                events.push(TrackEvent {
                    kind: TrackEventKind::Disappear,
                    track: t.clone(),
                });
            }
            alive
        });
        events
    }

    /// One analysed frame. `next_id` is the node-wide id counter.
    pub fn update(
        &mut self,
        cfg: &VisionTrackConfig,
        ts_ms: i64,
        dets: &[Detection],
        next_id: &mut u64,
    ) -> Vec<TrackEvent> {
        let mut events = self.expire(cfg, ts_ms);
        let predicted: Vec<[f32; 4]> = self.tracks.iter().map(|t| t.predicted(ts_ms)).collect();
        let mut track_used = vec![false; self.tracks.len()];
        let mut det_used = vec![false; dets.len()];
        for high in [true, false] {
            // shortcut: greedy best-IoU assignment instead of Hungarian, same
            // result on the sparse scenes of a camera; upgrade if crowds swap ids.
            let mut pairs: Vec<(f32, usize, usize)> = Vec::new();
            for (di, d) in dets.iter().enumerate() {
                if det_used[di] || (d.score >= cfg.high_score) != high {
                    continue;
                }
                for (ti, t) in self.tracks.iter().enumerate() {
                    if track_used[ti] || t.label != d.label {
                        continue;
                    }
                    let o = iou(&predicted[ti], &d.bbox);
                    if o >= cfg.min_iou {
                        pairs.push((o, ti, di));
                    }
                }
            }
            pairs.sort_by(|a, b| b.0.total_cmp(&a.0));
            for (_, ti, di) in pairs {
                if track_used[ti] || det_used[di] {
                    continue;
                }
                track_used[ti] = true;
                det_used[di] = true;
                let t = &mut self.tracks[ti];
                let d = &dets[di];
                let dt = (ts_ms - t.last_ts_ms) as f32;
                if dt > 0.0 {
                    let centre = |b: &[f32; 4]| [b[0] + b[2] / 2.0, b[1] + b[3] / 2.0];
                    let (c0, c1) = (centre(&t.bbox), centre(&d.bbox));
                    // Smoothed so one jittery box does not fling the prediction.
                    t.vel = [
                        0.5 * t.vel[0] + 0.5 * (c1[0] - c0[0]) / dt,
                        0.5 * t.vel[1] + 0.5 * (c1[1] - c0[1]) / dt,
                    ];
                }
                t.bbox = d.bbox;
                t.score = d.score;
                t.hits += 1;
                t.score_sum += f64::from(d.score);
                t.last_ts_ms = ts_ms;
                if !t.confirmed && t.hits >= cfg.min_hits {
                    t.confirmed = true;
                    events.push(TrackEvent {
                        kind: TrackEventKind::Appear,
                        track: t.clone(),
                    });
                }
            }
        }
        // A tentative track that missed this frame was noise.
        let mut i = 0;
        self.tracks.retain(|t| {
            let keep = t.confirmed || track_used[i];
            i += 1;
            keep
        });
        for (di, d) in dets.iter().enumerate() {
            if det_used[di] || d.score < cfg.high_score {
                continue;
            }
            *next_id += 1;
            let t = Track {
                id: *next_id,
                label: d.label.clone(),
                bbox: d.bbox,
                score: d.score,
                vel: [0.0, 0.0],
                hits: 1,
                confirmed: cfg.min_hits <= 1,
                first_ts_ms: ts_ms,
                last_ts_ms: ts_ms,
                score_sum: f64::from(d.score),
            };
            if t.confirmed {
                events.push(TrackEvent {
                    kind: TrackEventKind::Appear,
                    track: t.clone(),
                });
            }
            self.tracks.push(t);
        }
        events
    }

    pub fn confirmed(&self) -> impl Iterator<Item = &Track> {
        self.tracks.iter().filter(|t| t.confirmed)
    }
}

/// Tracker state of one source, plus what port 0 repeats on expiry.
#[derive(Default)]
struct SourceState {
    tracker: Tracker,
    width: u32,
    height: u32,
}

/// `(field, id)` of the source of a `vision-detect` message: a device
/// camera or a media stream (D175).
pub fn source_of(payload: &Value) -> Option<(&'static str, String)> {
    ["device_id", "stream"].into_iter().find_map(|f| {
        payload
            .get(f)
            .and_then(Value::as_str)
            .map(|id| (f, id.to_string()))
    })
}

/// Integer field, also from a float (a JS function node upstream sends
/// every number as f64).
pub fn int_field(payload: &Value, key: &str) -> Option<i64> {
    let v = payload.get(key)?;
    v.as_i64()
        .or_else(|| v.as_f64().filter(|f| f.is_finite()).map(|f| f as i64))
}

/// Port 0 payload: the confirmed tracks of a source.
fn tracks_payload(field: &str, id: &str, ts_ms: i64, st: &SourceState, frame: &Value) -> Value {
    let tracks: Vec<Value> = st.tracker.confirmed().map(Track::to_json).collect();
    let mut labels: Vec<&str> = st.tracker.confirmed().map(|t| t.label.as_str()).collect();
    labels.sort_unstable();
    labels.dedup();
    json!({
        field: id,
        "ts_ms": ts_ms,
        "width": st.width,
        "height": st.height,
        "frame_key": frame,
        "count": tracks.len(),
        "labels": labels,
        "tracks": tracks,
    })
}

fn message(payload: Value, topic: &str) -> MsgHandle {
    let mut body = std::collections::BTreeMap::new();
    body.insert("payload".to_string(), Variant::from(payload));
    body.insert("topic".to_string(), Variant::from(topic));
    MsgHandle::with_properties(body)
}

/// Pending output: port 0 state, then port 1 events.
type Outputs = Vec<(usize, Value, String)>;

#[flow_node("pnex-vision-track", red_name = "pnex-vision-track")]
struct VisionTrackNode {
    base: BaseFlowNodeState,
    config: VisionTrackConfig,
    /// Per `(field, id)` source.
    state: std::sync::Mutex<(HashMap<(&'static str, String), SourceState>, u64)>,
}

impl VisionTrackNode {
    fn build(
        _flow: &Flow,
        base_node: BaseFlowNodeState,
        config: &RedFlowNodeConfig,
        _options: Option<&config::Config>,
    ) -> Result<Box<dyn FlowNodeBehavior>> {
        let cfg = VisionTrackConfig::deserialize(&config.rest)
            .map_err(|e| EdgelinkError::BadFlowsJson(format!("{NODE} : invalid config : {e}")))?;
        if let Some((code, message)) = cfg.check() {
            return Err(EdgelinkError::BadFlowsJson(format!("{NODE} [{code}] : {message}")).into());
        }
        Ok(Box::new(VisionTrackNode {
            base: base_node,
            config: cfg,
            state: std::sync::Mutex::new((HashMap::new(), 0)),
        }))
    }

    fn event_outputs(field: &str, id: &str, events: &[TrackEvent], out: &mut Outputs) {
        for e in events {
            let mut v = e.to_json();
            v[field] = Value::from(id);
            out.push((1, v, id.to_string()));
        }
    }

    fn on_frame(&self, payload: &Value) -> Outputs {
        let (Some((field, id)), Some(ts_ms)) = (source_of(payload), int_field(payload, "ts_ms"))
        else {
            log::warn!(
                "{NODE} [{}] : message ignored — expected a vision-detect payload",
                self.name()
            );
            return Vec::new();
        };
        let dets: Vec<Detection> = payload
            .get("detections")
            .cloned()
            .and_then(|d| serde_json::from_value(d).ok())
            .unwrap_or_default();
        let mut out = Vec::new();
        let mut guard = self.state.lock().expect("track state");
        let (sources, next_id) = &mut *guard;
        let st = sources.entry((field, id.clone())).or_default();
        let dim = |k: &str| int_field(payload, k).map(|v| v.clamp(0, i64::from(u32::MAX)) as u32);
        st.width = dim("width").unwrap_or(st.width);
        st.height = dim("height").unwrap_or(st.height);
        let events = st.tracker.update(&self.config, ts_ms, &dets, next_id);
        let frame = payload.get("frame_key").cloned().unwrap_or(Value::Null);
        out.push((0, tracks_payload(field, &id, ts_ms, st, &frame), id.clone()));
        Self::event_outputs(field, &id, &events, &mut out);
        out
    }

    /// Expires the tracks of sources that stopped sending frames.
    fn sweep(&self, now_ms: i64) -> Outputs {
        let mut out = Vec::new();
        let mut guard = self.state.lock().expect("track state");
        guard.0.retain(|(field, id), st| {
            let events = st.tracker.expire(&self.config, now_ms);
            if !events.is_empty() {
                out.push((
                    0,
                    tracks_payload(field, id, now_ms, st, &Value::Null),
                    id.clone(),
                ));
                Self::event_outputs(field, id, &events, &mut out);
            }
            !st.tracker.tracks.is_empty()
        });
        out
    }

    async fn send(&self, outputs: Outputs, cancel: CancellationToken) -> Result<()> {
        for (port, payload, topic) in outputs {
            self.fan_out_one(
                Envelope {
                    port,
                    msg: message(payload, &topic),
                },
                cancel.clone(),
            )
            .await?;
        }
        Ok(())
    }
}

#[async_trait]
impl FlowNodeBehavior for VisionTrackNode {
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
                        _ = tokio::time::sleep(SWEEP) => {}
                    }
                    let out = node.sweep(chrono::Utc::now().timestamp_millis());
                    if let Err(e) = node.send(out, stop.child_token()).await {
                        log::warn!("{NODE} [{}] : expiry output failed: {e}", node.name());
                    }
                }
            });
        }
        while !stop_token.is_cancelled() {
            let cancel = stop_token.child_token();
            with_uow(
                self.as_ref(),
                cancel.child_token(),
                |node: &VisionTrackNode, msg: MsgHandle| async move {
                    let payload = {
                        let m = msg.read().await;
                        m.get("payload")
                            .map(|v| serde_json::to_value(v).unwrap_or(Value::Null))
                            .unwrap_or(Value::Null)
                    };
                    let out = node.on_frame(&payload);
                    node.send(out, cancel.child_token()).await
                },
            )
            .await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn det(label: &str, score: f32, x: f32) -> Detection {
        Detection {
            label: label.into(),
            class_id: 0,
            score,
            bbox: [x, 10.0, 40.0, 80.0],
        }
    }

    fn kinds(events: &[TrackEvent]) -> Vec<(TrackEventKind, u64)> {
        events.iter().map(|e| (e.kind, e.track.id)).collect()
    }

    #[test]
    fn appear_after_min_hits_then_disappear() {
        let cfg = VisionTrackConfig::default();
        let mut tr = Tracker::default();
        let mut id = 0;
        assert!(tr
            .update(&cfg, 0, &[det("person", 0.9, 0.0)], &mut id)
            .is_empty());
        let ev = tr.update(&cfg, 1000, &[det("person", 0.9, 5.0)], &mut id);
        assert_eq!(kinds(&ev), vec![(TrackEventKind::Appear, 1)]);
        // Moving steadily: predicted forward, same id.
        for k in 2..6 {
            let ev = tr.update(
                &cfg,
                k * 1000,
                &[det("person", 0.9, 5.0 * k as f32)],
                &mut id,
            );
            assert!(ev.is_empty());
        }
        assert_eq!(tr.confirmed().count(), 1);
        let ev = tr.expire(&cfg, 5000 + 3001);
        assert_eq!(kinds(&ev), vec![(TrackEventKind::Disappear, 1)]);
        let gone = &ev[0].track;
        assert_eq!((gone.first_ts_ms, gone.last_ts_ms), (0, 5000));
        assert!(tr.tracks.is_empty());
    }

    #[test]
    fn low_scores_extend_but_never_open() {
        let cfg = VisionTrackConfig::default();
        let mut tr = Tracker::default();
        let mut id = 0;
        assert!(tr
            .update(&cfg, 0, &[det("car", 0.3, 0.0)], &mut id)
            .is_empty());
        assert!(tr.tracks.is_empty(), "a weak detection opens nothing");
        tr.update(&cfg, 0, &[det("car", 0.9, 0.0)], &mut id);
        tr.update(&cfg, 100, &[det("car", 0.9, 0.0)], &mut id);
        tr.update(&cfg, 200, &[det("car", 0.2, 1.0)], &mut id);
        assert_eq!(tr.tracks[0].last_ts_ms, 200, "kept alive by stage 2");
        assert_eq!(id, 1);
    }

    #[test]
    fn labels_never_mix_and_noise_is_dropped() {
        let cfg = VisionTrackConfig::default();
        let mut tr = Tracker::default();
        let mut id = 0;
        tr.update(&cfg, 0, &[det("dog", 0.9, 0.0)], &mut id);
        // Same place, other label: the tentative dog misses → dropped.
        tr.update(&cfg, 100, &[det("cat", 0.9, 0.0)], &mut id);
        assert_eq!(tr.tracks.len(), 1);
        assert_eq!(tr.tracks[0].label, "cat");
        assert_eq!(tr.tracks[0].id, 2);
    }

    #[test]
    fn two_objects_keep_their_ids() {
        let cfg = VisionTrackConfig::default();
        let mut tr = Tracker::default();
        let mut id = 0;
        let frame = |a: f32, b: f32| vec![det("person", 0.9, a), det("person", 0.9, b)];
        tr.update(&cfg, 0, &frame(0.0, 200.0), &mut id);
        tr.update(&cfg, 100, &frame(4.0, 196.0), &mut id);
        tr.update(&cfg, 200, &frame(8.0, 192.0), &mut id);
        let mut by_x: Vec<(u64, f32)> = tr.confirmed().map(|t| (t.id, t.bbox[0])).collect();
        by_x.sort_by(|a, b| a.1.total_cmp(&b.1));
        assert_eq!(by_x, vec![(1, 8.0), (2, 192.0)]);
    }

    #[test]
    fn int_from_float() {
        assert_eq!(int_field(&json!({"t": 12.0}), "t"), Some(12));
        assert_eq!(int_field(&json!({"t": 12}), "t"), Some(12));
        assert_eq!(int_field(&json!({"t": "12"}), "t"), None);
    }

    #[test]
    fn source_field() {
        assert_eq!(
            source_of(&json!({"stream": "radio"})),
            Some(("stream", "radio".to_string()))
        );
        assert_eq!(source_of(&json!({})), None);
    }
}
