//! Continuous recording player (D80): plays a chain of MJPEG-AVI segments
//! back to back as ONE recording. Each segment is fetched and indexed with
//! `pnex_core::avi::read_avi`; the current one and the next are kept in
//! memory (prefetch — no stall at the minute boundary), older ones are
//! dropped. Only the displayed frame is materialized as an image URL (blob
//! on web, data URI on native), the previous one is revoked.
//!
//! The parent drives seeking through `seek` (wall-clock ms) and reads the
//! playhead back from `position` (timeline cursor, export window).
//!
//! Annotation layers (D105): for the ticked `layers`, the boxes of the
//! current and next segment are fetched with them and drawn as an SVG
//! overlay (same aspect ratio as the frame → aligned with `object-contain`).

use std::collections::HashMap;
use std::rc::Rc;

use dioxus::prelude::*;
use dioxus_i18n::t;
use pnex_core::avi::{read_avi, AviIndex};
use pnex_core::camera::{annotation_at, FrameAnnotation, VideoSegment};

use crate::api;
use crate::components::icons;
use crate::util::sleep;

/// Loaded segment: file bytes + frame index.
#[derive(Clone)]
struct Loaded {
    bytes: Rc<Vec<u8>>,
    index: Rc<AviIndex>,
}

/// `(start_ms, end_ms)` of a segment, `None` on unparsable timestamps.
pub(super) fn span_ms(seg: &VideoSegment) -> Option<(i64, i64)> {
    let start = chrono::DateTime::parse_from_rfc3339(&seg.started_at).ok()?;
    let end = chrono::DateTime::parse_from_rfc3339(&seg.ended_at).ok()?;
    Some((
        start.timestamp_millis(),
        end.timestamp_millis().max(start.timestamp_millis()),
    ))
}

/// Chain index to play for a wall-clock target: the segment covering it,
/// else the first one starting after it, else the last one.
pub(super) fn locate(spans: &[(i64, i64)], target_ms: i64) -> Option<usize> {
    if spans.is_empty() {
        return None;
    }
    spans
        .iter()
        .position(|&(s, e)| target_ms >= s && target_ms <= e)
        .or_else(|| spans.iter().position(|&(s, _)| s > target_ms))
        .or(Some(spans.len() - 1))
}

/// Frame index of `target_ms` inside a segment of `frames` frames.
pub(super) fn frame_at(span: (i64, i64), frames: usize, target_ms: i64) -> usize {
    if frames == 0 {
        return 0;
    }
    let (s, e) = span;
    if e <= s || target_ms <= s {
        return 0;
    }
    let ratio = ((target_ms - s) as f64 / (e - s) as f64).clamp(0.0, 1.0);
    ((ratio * (frames - 1) as f64).round() as usize).min(frames - 1)
}

/// Wall-clock ms of a frame (frames spread evenly over the segment span).
pub(super) fn frame_time(span: (i64, i64), frames: usize, idx: usize) -> i64 {
    let (s, e) = span;
    if frames <= 1 {
        return s;
    }
    s + ((e - s) as f64 * idx as f64 / (frames - 1) as f64).round() as i64
}

/// Playback period in ms for a recorded fps and a speed factor (clamped
/// to 1..=30 fps recorded, never faster than 20 ms per frame).
pub(super) fn frame_period_ms(fps: f64, speed: u32) -> u64 {
    let fps = if fps.is_finite() && fps > 0.0 {
        fps.clamp(1.0, 30.0)
    } else {
        5.0
    };
    ((1000.0 / fps / f64::from(speed.max(1))).round() as u64).max(20)
}

/// Playback speeds offered.
const SPEEDS: [u32; 4] = [1, 2, 4, 8];

/// Longest distance between a shown frame and the annotation drawn on it
/// (detections run at ~1 fps: a box stays for the frames around it).
const ANNOTATION_TOLERANCE_MS: i64 = 750;

/// Layer colors, by rank in the selection (full literals).
pub(super) const LAYER_COLORS: [&str; 7] = [
    "#ef4444", "#3b82f6", "#22c55e", "#f59e0b", "#a855f7", "#ec4899", "#14b8a6",
];

/// Color of the n-th selected layer.
pub(super) fn layer_color(rank: usize) -> &'static str {
    LAYER_COLORS[rank % LAYER_COLORS.len()]
}

/// Annotations of one segment for one layer selection, per layer.
type SegmentAnnotations = Rc<HashMap<String, Vec<FrameAnnotation>>>;

/// Groups annotations by layer (input ascending → each group ascending).
fn by_layer(anns: Vec<FrameAnnotation>) -> HashMap<String, Vec<FrameAnnotation>> {
    let mut out: HashMap<String, Vec<FrameAnnotation>> = HashMap::new();
    for a in anns {
        out.entry(a.layer_id.clone()).or_default().push(a);
    }
    out
}

/// RFC 3339 of unix ms.
fn rfc3339(ms: i64) -> String {
    chrono::DateTime::from_timestamp_millis(ms)
        .map(|t| t.to_rfc3339())
        .unwrap_or_default()
}

#[component]
pub(super) fn RecordingPlayer(
    /// Camera (annotation reads).
    device: i64,
    /// Chained segments (ascending, no overlap).
    segments: Vec<VideoSegment>,
    /// Ticked annotation layers (ids, in display order).
    layers: Signal<Vec<String>>,
    /// Seek request (wall-clock ms), consumed by the player.
    seek: Signal<Option<i64>>,
    /// Current playhead (wall-clock ms), written by the player.
    position: Signal<Option<i64>>,
) -> Element {
    let spans: Rc<Vec<(i64, i64)>> = Rc::new(
        segments
            .iter()
            .map(|s| span_ms(s).unwrap_or((0, 0)))
            .collect(),
    );
    let ids: Rc<Vec<String>> = Rc::new(segments.iter().map(|s| s.id.clone()).collect());
    let count = segments.len();

    let mut cache = use_signal(HashMap::<usize, Loaded>::new);
    let mut pending = use_signal(std::collections::HashSet::<usize>::new);
    let mut failed = use_signal(std::collections::HashSet::<usize>::new);
    let mut cur = use_signal(|| 0usize);
    let mut idx = use_signal(|| 0usize);
    // Frame to jump to once `cur` is loaded (seek into a fresh segment).
    let mut want_ms = use_signal(|| None::<i64>);
    let mut playing = use_signal(|| true);
    let mut speed = use_signal(|| 1u32);
    let mut url = use_signal(|| None::<String>);
    // (segment index, layer selection) → boxes, and in-flight reads.
    let mut anns = use_signal(HashMap::<(usize, String), SegmentAnnotations>::new);
    let mut anns_pending = use_signal(std::collections::HashSet::<(usize, String)>::new);

    // Seek requests from the parent (timeline).
    let spans_seek = spans.clone();
    use_effect(move || {
        let Some(target) = seek() else {
            return;
        };
        seek.set(None);
        if let Some(i) = locate(&spans_seek, target) {
            cur.set(i);
            idx.set(0);
            want_ms.set(Some(target));
        }
    });

    // Loader: current + next segment, drop the others.
    let ids_load = ids.clone();
    use_effect(move || {
        let c = cur();
        let wanted: Vec<usize> = [c, c + 1].into_iter().filter(|&i| i < count).collect();
        cache.with_mut(|m| m.retain(|k, _| wanted.contains(k)));
        for i in wanted {
            if cache.peek().contains_key(&i)
                || pending.peek().contains(&i)
                || failed.peek().contains(&i)
            {
                continue;
            }
            pending.with_mut(|p| p.insert(i));
            let id = ids_load[i].clone();
            spawn(async move {
                let result = api::cameras::segment_bytes(&id).await;
                pending.with_mut(|p| p.remove(&i));
                let loaded = result.ok().and_then(|bytes| match read_avi(&bytes) {
                    Ok(index) if !index.frames.is_empty() => Some(Loaded {
                        bytes: Rc::new(bytes),
                        index: Rc::new(index),
                    }),
                    _ => None,
                });
                match loaded {
                    // Still wanted (the user may have seeked away meanwhile).
                    Some(l) if i == *cur.peek() || i == *cur.peek() + 1 => {
                        cache.with_mut(|m| m.insert(i, l));
                    }
                    Some(_) => {}
                    None => {
                        failed.with_mut(|f| f.insert(i));
                    }
                }
            });
        }
    });

    // Annotation loader: boxes of the ticked layers for the current and
    // next segment (a changed selection is a new cache key).
    let spans_anns = spans.clone();
    use_effect(move || {
        let c = cur();
        let selected = layers();
        if selected.is_empty() {
            return;
        }
        let sel_key = selected.join(",");
        anns.with_mut(|m| m.retain(|(i, k), _| *k == sel_key && (*i == c || *i == c + 1)));
        for i in [c, c + 1].into_iter().filter(|&i| i < count) {
            let key = (i, sel_key.clone());
            if anns.peek().contains_key(&key) || anns_pending.peek().contains(&key) {
                continue;
            }
            anns_pending.with_mut(|p| p.insert(key.clone()));
            let (start, end) = spans_anns[i];
            let selected = selected.clone();
            spawn(async move {
                // Margin: a detection a bit before/after the segment edges.
                let res = api::cameras::annotations(
                    device,
                    &rfc3339(start - ANNOTATION_TOLERANCE_MS),
                    &rfc3339(end + ANNOTATION_TOLERANCE_MS),
                    &selected,
                )
                .await;
                anns_pending.with_mut(|p| p.remove(&key));
                // Unreadable = no overlay (never an error for the playback).
                let grouped = Rc::new(by_layer(res.unwrap_or_default()));
                anns.with_mut(|m| m.insert(key, grouped));
            });
        }
    });

    // Pending seek resolved once the target segment is in memory.
    let spans_want = spans.clone();
    use_effect(move || {
        let Some(target) = want_ms() else {
            return;
        };
        let c = cur();
        let Some(frames) = cache.read().get(&c).map(|l| l.index.frames.len()) else {
            return;
        };
        idx.set(frame_at(spans_want[c], frames, target));
        want_ms.set(None);
    });

    // Current frame → image URL (previous one released) + playhead.
    let spans_frame = spans.clone();
    use_effect(move || {
        let c = cur();
        let i = idx();
        let Some(data) = cache.read().get(&c).cloned() else {
            return;
        };
        let Some(&(start, len)) = data.index.frames.get(i) else {
            return;
        };
        let Some(jpeg) = data.bytes.get(start..start + len) else {
            return;
        };
        let next = crate::util::image_url_from_bytes(jpeg, "image/jpeg");
        if let Some(prev) = url.peek().clone() {
            crate::util::release_image_url(&prev);
        }
        url.set(next);
        position.set(Some(frame_time(spans_frame[c], data.index.frames.len(), i)));
    });
    use_drop(move || {
        if let Some(prev) = url.peek().clone() {
            crate::util::release_image_url(&prev);
        }
    });

    // Playback clock: next frame, then next segment (waits while it loads,
    // skips unreadable ones), stops after the last one.
    use_future(move || async move {
        loop {
            let c = *cur.peek();
            let fps = cache.peek().get(&c).map(|l| l.index.fps).unwrap_or(5.0);
            sleep(std::time::Duration::from_millis(frame_period_ms(
                fps,
                *speed.peek(),
            )))
            .await;
            if !*playing.peek() || want_ms.peek().is_some() {
                continue;
            }
            let c = *cur.peek();
            let Some(total) = cache.peek().get(&c).map(|l| l.index.frames.len()) else {
                if failed.peek().contains(&c) && c + 1 < count {
                    cur.set(c + 1);
                    idx.set(0);
                }
                continue;
            };
            let i = *idx.peek();
            if i + 1 < total {
                idx.set(i + 1);
            } else if c + 1 < count {
                let next_ready =
                    cache.peek().contains_key(&(c + 1)) || failed.peek().contains(&(c + 1));
                if next_ready {
                    cur.set(c + 1);
                    idx.set(0);
                }
            } else {
                playing.set(false);
            }
        }
    });

    let c = cur();
    let buffering = !cache.read().contains_key(&c) && !failed.read().contains(&c);
    let unreadable = failed.read().contains(&c);
    let time_label = position()
        .and_then(|ms| chrono::DateTime::from_timestamp_millis(ms))
        .map(|t| {
            t.with_timezone(&chrono::Local)
                .format("%H:%M:%S")
                .to_string()
        })
        .unwrap_or_default();
    let dims = cache
        .read()
        .get(&c)
        .map(|l| (l.index.width, l.index.height, l.index.fps));
    // Boxes of each ticked layer for the frame on screen.
    let selected = layers();
    let overlays: Vec<(String, &'static str, FrameAnnotation)> = match position() {
        Some(t) if !selected.is_empty() => {
            let key = (c, selected.join(","));
            let guard = anns.read();
            match guard.get(&key) {
                Some(groups) => selected
                    .iter()
                    .enumerate()
                    .filter_map(|(rank, id)| {
                        let found = groups
                            .get(id)
                            .and_then(|g| annotation_at(g, t, ANNOTATION_TOLERANCE_MS))?;
                        Some((id.clone(), layer_color(rank), found.clone()))
                    })
                    .collect(),
                None => Vec::new(),
            }
        }
        _ => Vec::new(),
    };

    rsx! {
        div { class: "space-y-3",
            div { class: "relative aspect-video bg-gray-900 rounded-lg overflow-hidden flex items-center justify-center",
                if let Some(src) = url() {
                    img { class: "w-full h-full object-contain", src: "{src}", alt: "" }
                }
                for (id, color, ann) in overlays {
                    svg {
                        key: "{id}",
                        class: "absolute inset-0 w-full h-full pointer-events-none",
                        view_box: "0 0 {ann.width.max(1)} {ann.height.max(1)}",
                        preserve_aspect_ratio: "xMidYMid meet",
                        for (i, d) in ann.detections.iter().enumerate() {
                            g { key: "{i}",
                                rect {
                                    x: "{d.bbox[0]}",
                                    y: "{d.bbox[1]}",
                                    width: "{d.bbox[2]}",
                                    height: "{d.bbox[3]}",
                                    fill: "none",
                                    stroke: color,
                                    stroke_width: "{(ann.width.max(1) as f32 / 200.0).max(1.0)}",
                                }
                                text {
                                    x: "{d.bbox[0] + 2.0}",
                                    y: "{(d.bbox[1] - 3.0).max(10.0)}",
                                    fill: color,
                                    font_size: "{(ann.width.max(1) as f32 / 40.0).max(8.0)}",
                                    font_weight: "600",
                                    "{d.label} {(d.score * 100.0).round()}%"
                                }
                            }
                        }
                    }
                }
                if buffering {
                    span { class: "absolute animate-spin inline-block rounded-full h-8 w-8 border-b-2 border-white" }
                }
                if unreadable {
                    p { class: "absolute inset-x-0 bottom-2 text-center text-xs text-red-300",
                        {t!("cameras-player-unreadable")}
                    }
                }
                if !time_label.is_empty() {
                    span { class: "absolute top-2 left-2 px-2 py-0.5 rounded bg-black/60 text-white text-xs tabular-nums",
                        "{time_label}"
                    }
                }
            }
            div { class: "flex flex-wrap items-center gap-2",
                button {
                    class: "px-2 py-1.5 text-sm text-gray-700 border border-gray-300 rounded-lg hover:bg-gray-50 disabled:opacity-40",
                    r#type: "button",
                    disabled: c == 0,
                    title: t!("cameras-player-prev"),
                    onclick: move |_| {
                        let c = cur();
                        if c > 0 {
                            cur.set(c - 1);
                            idx.set(0);
                        }
                    },
                    icons::ChevronLeft { class: "h-4 w-4" }
                }
                button {
                    class: "px-3 py-1.5 text-sm text-white bg-blue-600 rounded-lg hover:bg-blue-700 min-w-20",
                    r#type: "button",
                    onclick: move |_| {
                        let next = !playing();
                        playing.set(next);
                    },
                    if playing() {
                        {t!("cameras-player-pause")}
                    } else {
                        {t!("cameras-player-play")}
                    }
                }
                button {
                    class: "px-2 py-1.5 text-sm text-gray-700 border border-gray-300 rounded-lg hover:bg-gray-50 disabled:opacity-40",
                    r#type: "button",
                    disabled: c + 1 >= count,
                    title: t!("cameras-player-next"),
                    onclick: move |_| {
                        let c = cur();
                        if c + 1 < count {
                            cur.set(c + 1);
                            idx.set(0);
                        }
                    },
                    icons::ChevronRight { class: "h-4 w-4" }
                }
                div { class: "flex rounded-lg border border-gray-300 overflow-hidden",
                    for s in SPEEDS {
                        button {
                            key: "{s}",
                            class: if speed() == s { "px-2 py-1.5 text-xs bg-gray-800 text-white" } else { "px-2 py-1.5 text-xs text-gray-700 hover:bg-gray-50" },
                            r#type: "button",
                            onclick: move |_| speed.set(s),
                            "{s}×"
                        }
                    }
                }
                span { class: "ml-auto text-xs text-gray-500 tabular-nums",
                    {t!("cameras-player-segment", index: c + 1, total: count)}
                    if let Some((w, h, fps)) = dims {
                        " · "
                        {t!("cameras-player-info", width: w, height: h, fps: format!("{fps:.1}"))}
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frame_period_is_clamped() {
        assert_eq!(frame_period_ms(5.0, 1), 200);
        assert_eq!(frame_period_ms(0.0, 1), 200);
        assert_eq!(frame_period_ms(1000.0, 1), 33);
        assert_eq!(frame_period_ms(0.2, 1), 1000);
        assert_eq!(frame_period_ms(5.0, 4), 50);
        assert_eq!(frame_period_ms(30.0, 8), 20);
    }

    #[test]
    fn locate_and_frames() {
        let spans = [(0, 60_000), (60_000, 120_000), (300_000, 360_000)];
        assert_eq!(locate(&spans, 30_000), Some(0));
        assert_eq!(locate(&spans, 90_000), Some(1));
        // In a gap → next segment; after the end → last one.
        assert_eq!(locate(&spans, 200_000), Some(2));
        assert_eq!(locate(&spans, 999_000), Some(2));
        assert_eq!(locate(&[], 0), None);
        assert_eq!(frame_at((0, 60_000), 301, 30_000), 150);
        assert_eq!(frame_at((0, 60_000), 301, 90_000), 300);
        assert_eq!(frame_time((0, 60_000), 301, 150), 30_000);
    }
}
