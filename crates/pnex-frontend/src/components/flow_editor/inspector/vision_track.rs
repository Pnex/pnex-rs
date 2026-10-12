use super::helpers::*;
use super::*;

use crate::components::ui::FIELD_LABEL;
use pnex_core::vision::{VisionTrackConfig, TRACK_LOST_MAX_SECS};

// ─────────────── vision-track (media-vision-studio.md D192) ───────────────

/// Writes one numeric field of the selected track node.
type TrackSetter = fn(&mut VisionTrackConfig, f64);

/// Number input committed when it parses inside `range`.
fn track_field(
    mut cx: EditorCx,
    (label, hint): (String, String),
    mut raw: Signal<String>,
    range: (f64, f64),
    step: &'static str,
    can_write: bool,
    set: TrackSetter,
) -> Element {
    let invalid = parse_secs(&raw()).is_none_or(|v| !(range.0..=range.1).contains(&v));
    rsx! {
        label { class: "block",
            span { class: FIELD_LABEL, {label} }
            input {
                class: if invalid { "w-full px-2 py-1.5 border border-red-400 bg-red-50 rounded-lg text-sm" } else { "w-full px-2 py-1.5 border border-gray-300 rounded-lg text-sm" },
                r#type: "number",
                min: "{range.0}",
                max: "{range.1}",
                step,
                value: "{raw}",
                disabled: !can_write,
                oninput: move |event| {
                    let next = event.value();
                    raw.set(next.clone());
                    if let Some(v) = parse_secs(&next).filter(|v| (range.0..=range.1).contains(v)) {
                        patch_selected(
                            &mut cx,
                            move |node: &mut FlowNode| {
                                if let FlowNodeKind::VisionTrack { config } = &mut node.kind {
                                    set(config, v);
                                }
                            },
                        );
                    }
                },
            }
            span { class: "text-xs text-gray-500 mt-1 block", {hint} }
        }
    }
}

/// Vision track inspector: the four tracker knobs, defaults tuned for a
/// 1 fps detection.
#[component]
pub(super) fn VisionTrackForm(
    cx: EditorCx,
    initial: VisionTrackConfig,
    can_write: bool,
) -> Element {
    let high = use_signal(|| initial.high_score.to_string());
    let iou = use_signal(|| initial.min_iou.to_string());
    let hits = use_signal(|| initial.min_hits.to_string());
    let lost = use_signal(|| v_to_string(initial.lost_after_secs));

    rsx! {
        div { class: "space-y-3",
            p { class: "text-xs text-gray-500", {t!("flows-track-help")} }
            {
                track_field(
                    cx,
                    (
                        t!("flows-track-high-score").to_string(),
                        t!("flows-track-high-score-hint").to_string(),
                    ),
                    high,
                    (0.0, 1.0),
                    "0.05",
                    can_write,
                    |c, v| c.high_score = v as f32,
                )
            }
            {
                track_field(
                    cx,
                    (
                        t!("flows-track-min-iou").to_string(),
                        t!("flows-track-min-iou-hint").to_string(),
                    ),
                    iou,
                    (0.01, 1.0),
                    "0.05",
                    can_write,
                    |c, v| c.min_iou = v as f32,
                )
            }
            {
                track_field(
                    cx,
                    (
                        t!("flows-track-min-hits").to_string(),
                        t!("flows-track-min-hits-hint").to_string(),
                    ),
                    hits,
                    (1.0, 100.0),
                    "1",
                    can_write,
                    |c, v| c.min_hits = v.round() as u32,
                )
            }
            {
                track_field(
                    cx,
                    (
                        t!("flows-track-lost-after").to_string(),
                        t!("flows-track-lost-after-hint").to_string(),
                    ),
                    lost,
                    (0.1, TRACK_LOST_MAX_SECS),
                    "0.5",
                    can_write,
                    |c, v| c.lost_after_secs = v,
                )
            }
        }
    }
}
