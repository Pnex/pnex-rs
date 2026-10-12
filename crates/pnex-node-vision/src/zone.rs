//! `pnex-vision-zone` node (media-vision-studio.md D193) — enter / exit
//! events of the `vision-track` objects in zones drawn over the image.
//!
//! - Zones are polygons in normalized coordinates (0–1), tested against the
//!   box anchor (bottom centre by default) divided by the frame size.
//! - An object enters once it has stayed `min_dwell_secs` inside (0 = at
//!   once); it exits when its anchor leaves the zone or its track is gone
//!   (the tracker repeats its state when tracks expire). Only an object
//!   that entered can exit.
//! - Output (port 0), one message per event: `{event, zone, track_id,
//!   label, ts_ms, t0_ms, duration_s (exit), count, device_id | stream}` —
//!   `count` = objects in the zone right after the event.

use std::collections::HashMap;
use std::sync::Arc;

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

use pnex_core::vision::{point_in_polygon, VisionZoneConfig, ZoneAnchor};

use crate::track::{int_field, source_of};

const NODE: &str = "pnex-vision-zone";

/// A tracked object as read from the `vision-track` port 0 payload.
#[derive(Debug, Clone, Deserialize)]
pub struct TrackView {
    pub track_id: u64,
    pub label: String,
    pub bbox: [f32; 4],
}

#[derive(Debug, Clone)]
struct Member {
    since_ms: i64,
    entered: bool,
    label: String,
}

/// Zone memberships of one source, keyed by `(zone index, track id)`.
#[derive(Debug, Default)]
pub struct ZoneState {
    members: HashMap<(usize, u64), Member>,
}

fn anchor_of(b: &[f32; 4], anchor: ZoneAnchor, w: f32, h: f32) -> [f32; 2] {
    let y = match anchor {
        ZoneAnchor::Bottom => b[1] + b[3],
        ZoneAnchor::Center => b[1] + b[3] / 2.0,
    };
    [(b[0] + b[2] / 2.0) / w, y / h]
}

impl ZoneState {
    fn count(&self, zone: usize) -> usize {
        self.members
            .iter()
            .filter(|((z, _), m)| *z == zone && m.entered)
            .count()
    }

    /// Applies the current tracks of the source; returns the events in order.
    pub fn update(
        &mut self,
        cfg: &VisionZoneConfig,
        ts_ms: i64,
        width: u32,
        height: u32,
        tracks: &[TrackView],
    ) -> Vec<Value> {
        let dwell_ms = (cfg.min_dwell_secs * 1000.0) as i64;
        let (w, h) = (width.max(1) as f32, height.max(1) as f32);
        let mut events = Vec::new();
        for (zi, zone) in cfg.zones.iter().enumerate() {
            let inside: HashMap<u64, &TrackView> = tracks
                .iter()
                .filter(|t| cfg.labels.is_empty() || cfg.labels.contains(&t.label))
                .filter(|t| point_in_polygon(anchor_of(&t.bbox, cfg.anchor, w, h), &zone.points))
                .map(|t| (t.track_id, t))
                .collect();
            // Exits first, so `count` of a simultaneous enter is right.
            let mut gone: Vec<u64> = self
                .members
                .keys()
                .filter(|(z, id)| *z == zi && !inside.contains_key(id))
                .map(|(_, id)| *id)
                .collect();
            gone.sort_unstable();
            for id in gone {
                let m = self.members.remove(&(zi, id)).expect("member");
                if m.entered {
                    events.push(json!({
                        "event": "exit",
                        "zone": zone.name,
                        "track_id": id,
                        "label": m.label,
                        "ts_ms": ts_ms,
                        "t0_ms": m.since_ms,
                        "duration_s": (ts_ms - m.since_ms) as f64 / 1000.0,
                        "count": self.count(zi),
                    }));
                }
            }
            let mut ids: Vec<&u64> = inside.keys().collect();
            ids.sort_unstable();
            for id in ids {
                let t = inside[id];
                let m = self.members.entry((zi, *id)).or_insert(Member {
                    since_ms: ts_ms,
                    entered: false,
                    label: t.label.clone(),
                });
                if !m.entered && ts_ms - m.since_ms >= dwell_ms {
                    m.entered = true;
                    let since = m.since_ms;
                    events.push(json!({
                        "event": "enter",
                        "zone": zone.name,
                        "track_id": id,
                        "label": t.label,
                        "ts_ms": ts_ms,
                        "t0_ms": since,
                        "count": self.count(zi),
                    }));
                }
            }
        }
        events
    }
}

#[flow_node("pnex-vision-zone", red_name = "pnex-vision-zone")]
struct VisionZoneNode {
    base: BaseFlowNodeState,
    config: VisionZoneConfig,
    state: std::sync::Mutex<HashMap<(&'static str, String), ZoneState>>,
}

impl VisionZoneNode {
    fn build(
        _flow: &Flow,
        base_node: BaseFlowNodeState,
        config: &RedFlowNodeConfig,
        _options: Option<&config::Config>,
    ) -> Result<Box<dyn FlowNodeBehavior>> {
        let cfg = VisionZoneConfig::deserialize(&config.rest)
            .map_err(|e| EdgelinkError::BadFlowsJson(format!("{NODE} : invalid config : {e}")))?;
        if let Some((code, message)) = cfg.check() {
            return Err(EdgelinkError::BadFlowsJson(format!("{NODE} [{code}] : {message}")).into());
        }
        Ok(Box::new(VisionZoneNode {
            base: base_node,
            config: cfg,
            state: std::sync::Mutex::new(HashMap::new()),
        }))
    }

    fn on_tracks(&self, payload: &Value) -> Vec<(Value, String)> {
        let (Some((field, id)), Some(ts_ms), Some(tracks)) = (
            source_of(payload),
            int_field(payload, "ts_ms"),
            payload
                .get("tracks")
                .cloned()
                .and_then(|t| serde_json::from_value::<Vec<TrackView>>(t).ok()),
        ) else {
            log::warn!(
                "{NODE} [{}] : message ignored — expected a vision-track payload (port 0)",
                self.name()
            );
            return Vec::new();
        };
        let dim = |k: &str| {
            int_field(payload, k)
                .unwrap_or(0)
                .clamp(0, i64::from(u32::MAX)) as u32
        };
        let (w, h) = (dim("width"), dim("height"));
        if w == 0 || h == 0 {
            log::warn!(
                "{NODE} [{}] : message ignored — frame size unknown",
                self.name()
            );
            return Vec::new();
        }
        let mut sources = self.state.lock().expect("zone state");
        let st = sources.entry((field, id.clone())).or_default();
        st.update(&self.config, ts_ms, w, h, &tracks)
            .into_iter()
            .map(|mut e| {
                e[field] = Value::from(id.as_str());
                (e, id.clone())
            })
            .collect()
    }
}

#[async_trait]
impl FlowNodeBehavior for VisionZoneNode {
    fn get_base(&self) -> &BaseFlowNodeState {
        &self.base
    }

    async fn run(self: Arc<Self>, stop_token: CancellationToken) {
        while !stop_token.is_cancelled() {
            let cancel = stop_token.child_token();
            with_uow(
                self.as_ref(),
                cancel.child_token(),
                |node: &VisionZoneNode, msg: MsgHandle| async move {
                    let payload = {
                        let m = msg.read().await;
                        m.get("payload")
                            .map(|v| serde_json::to_value(v).unwrap_or(Value::Null))
                            .unwrap_or(Value::Null)
                    };
                    for (event, topic) in node.on_tracks(&payload) {
                        let mut body = std::collections::BTreeMap::new();
                        body.insert("payload".to_string(), Variant::from(event));
                        body.insert("topic".to_string(), Variant::from(topic));
                        node.fan_out_one(
                            Envelope {
                                port: 0,
                                msg: MsgHandle::with_properties(body),
                            },
                            cancel.child_token(),
                        )
                        .await?;
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
    use pnex_core::vision::VisionZone;

    fn cfg(dwell: f64) -> VisionZoneConfig {
        VisionZoneConfig {
            // Left half of the image.
            zones: vec![VisionZone {
                name: "dock".into(),
                points: vec![[0.0, 0.0], [0.5, 0.0], [0.5, 1.0], [0.0, 1.0]],
            }],
            min_dwell_secs: dwell,
            ..Default::default()
        }
    }

    fn at(id: u64, x: f32) -> TrackView {
        TrackView {
            track_id: id,
            label: "person".into(),
            bbox: [x, 10.0, 20.0, 50.0],
        }
    }

    fn summary(events: &[Value]) -> Vec<(String, u64, u64)> {
        events
            .iter()
            .map(|e| {
                (
                    e["event"].as_str().unwrap().to_string(),
                    e["track_id"].as_u64().unwrap(),
                    e["count"].as_u64().unwrap(),
                )
            })
            .collect()
    }

    #[test]
    fn enter_count_exit() {
        let c = cfg(0.0);
        let mut st = ZoneState::default();
        let ev = st.update(&c, 0, 200, 100, &[at(1, 10.0), at(2, 150.0)]);
        assert_eq!(summary(&ev), vec![("enter".into(), 1, 1)]);
        let ev = st.update(&c, 1000, 200, 100, &[at(1, 10.0), at(2, 20.0)]);
        assert_eq!(summary(&ev), vec![("enter".into(), 2, 2)]);
        // Track 1 gone (expired), track 2 walks out.
        let ev = st.update(&c, 2000, 200, 100, &[at(2, 160.0)]);
        assert_eq!(
            summary(&ev),
            vec![("exit".into(), 1, 1), ("exit".into(), 2, 0)]
        );
        assert_eq!(ev[0]["duration_s"], 2.0);
    }

    #[test]
    fn dwell_delays_enter_and_short_visits_are_silent() {
        let c = cfg(5.0);
        let mut st = ZoneState::default();
        assert!(st.update(&c, 0, 200, 100, &[at(1, 10.0)]).is_empty());
        assert!(
            st.update(&c, 2000, 200, 100, &[]).is_empty(),
            "left before 5 s"
        );
        assert!(st.update(&c, 3000, 200, 100, &[at(1, 10.0)]).is_empty());
        let ev = st.update(&c, 8000, 200, 100, &[at(1, 10.0)]);
        assert_eq!(summary(&ev), vec![("enter".into(), 1, 1)]);
        assert_eq!(ev[0]["t0_ms"], 3000);
    }

    #[test]
    fn labels_and_anchor() {
        let mut c = cfg(0.0);
        c.labels = vec!["car".into()];
        let mut st = ZoneState::default();
        assert!(st.update(&c, 0, 200, 100, &[at(1, 10.0)]).is_empty());
        // Box top inside the top half, bottom outside: anchor decides.
        let mut c = VisionZoneConfig {
            zones: vec![VisionZone {
                name: "top".into(),
                points: vec![[0.0, 0.0], [1.0, 0.0], [1.0, 0.5], [0.0, 0.5]],
            }],
            ..Default::default()
        };
        let tall = TrackView {
            track_id: 7,
            label: "person".into(),
            bbox: [10.0, 10.0, 20.0, 70.0],
        };
        let mut st = ZoneState::default();
        assert!(st
            .update(&c, 0, 200, 100, std::slice::from_ref(&tall))
            .is_empty());
        c.anchor = ZoneAnchor::Center;
        let mut st = ZoneState::default();
        assert_eq!(st.update(&c, 0, 200, 100, &[tall]).len(), 1);
    }
}
