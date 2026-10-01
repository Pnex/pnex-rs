use super::helpers::*;
use super::*;

// ─────────────── video-record (camera-video.md D78/D79) ───────────────

/// Field keys of the video-record form (also the parse-error markers).
const F_SEGMENT_SECS: &str = "segment_secs";
const F_SEGMENT_MB: &str = "max_segment_mb";
const F_GAP_SECS: &str = "gap_secs";
const F_MAX_FPS: &str = "max_fps";
const F_RETENTION: &str = "retention_days";
const F_STREAM: &str = "stream";

/// Violation code of `VideoRecordConfig::check()` that points at a field.
fn field_of_code(code: &str) -> Option<&'static str> {
    match code {
        "video_segment_secs_invalid" => Some(F_SEGMENT_SECS),
        "video_segment_mb_invalid" => Some(F_SEGMENT_MB),
        "video_gap_secs_invalid" => Some(F_GAP_SECS),
        "camera_fps_invalid" => Some(F_MAX_FPS),
        "video_retention_invalid" => Some(F_RETENTION),
        "video_stream_too_long" => Some(F_STREAM),
        _ => None,
    }
}

/// Parses `raw` for `field` and patches the selected node; an unparsable
/// value leaves the config untouched and flags the field locally. Range
/// checks stay with `VideoRecordConfig::check()` (single source shared
/// with the save validation and the runtime).
fn apply_field(
    cx: &mut EditorCx,
    field: &'static str,
    raw: String,
    mut parse_errors: Signal<Vec<&'static str>>,
) {
    let trimmed = raw.trim().to_string();
    let as_u32 = trimmed.parse::<u32>().ok();
    let as_f64 = if trimmed.is_empty() {
        Some(0.0)
    } else {
        parse_secs(&trimmed)
    };
    let ok = match field {
        F_STREAM => true,
        F_MAX_FPS => as_f64.is_some(),
        _ => as_u32.is_some(),
    };
    parse_errors.with_mut(|errs| {
        errs.retain(|f| *f != field);
        if !ok {
            errs.push(field);
        }
    });
    if !ok {
        return;
    }
    patch_selected(cx, move |node: &mut FlowNode| {
        if let FlowNodeKind::VideoRecord { config } = &mut node.kind {
            match field {
                F_SEGMENT_SECS => config.segment_secs = as_u32.unwrap_or_default(),
                F_SEGMENT_MB => config.max_segment_mb = as_u32.unwrap_or_default(),
                F_GAP_SECS => config.gap_secs = as_u32.unwrap_or_default(),
                F_RETENTION => config.retention_days = as_u32.unwrap_or_default(),
                F_MAX_FPS => config.max_fps = as_f64.unwrap_or_default(),
                _ => config.stream = raw,
            }
        }
    });
}

/// One labeled input of the form (number or text), red when flagged.
fn field_input(
    label: String,
    hint: String,
    value: Signal<String>,
    numeric: bool,
    invalid: bool,
    disabled: bool,
    oninput: impl FnMut(FormEvent) + 'static,
) -> Element {
    rsx! {
        label { class: "block",
            span { class: "text-xs font-medium text-gray-500 mb-1 block", {label} }
            input {
                class: if invalid {
                    "w-full px-2 py-1.5 border border-red-400 bg-red-50 rounded-lg text-sm"
                } else {
                    "w-full px-2 py-1.5 border border-gray-300 rounded-lg text-sm"
                },
                r#type: if numeric { "number" } else { "text" },
                value: "{value}",
                disabled,
                oninput,
            }
            span { class: "text-xs text-gray-400 mt-1 block", {hint} }
        }
    }
}

/// Video record inspector: segment duration/size/gap, recording rate,
/// retention and stream name. Fields failing `check()` turn red; the
/// localized violation itself is listed under the form (inspector body).
#[component]
pub(super) fn VideoRecordForm(
    mut cx: EditorCx,
    initial: VideoRecordConfig,
    can_write: bool,
) -> Element {
    let segment_secs = use_signal(move || initial.segment_secs.to_string());
    let segment_mb = use_signal(move || initial.max_segment_mb.to_string());
    let gap_secs = use_signal(move || initial.gap_secs.to_string());
    let max_fps = use_signal(move || v_to_string(initial.max_fps));
    let retention = use_signal(move || initial.retention_days.to_string());
    let stream = use_signal(move || initial.stream.clone());
    let parse_errors = use_signal(Vec::<&'static str>::new);

    // Current config of the selected node → the field `check()` rejects.
    let current = use_memo(move || {
        let id = cx.selected_node.cloned();
        cx.graph
            .read()
            .nodes
            .iter()
            .find(|n| Some(&n.id) == id.as_ref())
            .and_then(|n| match &n.kind {
                FlowNodeKind::VideoRecord { config } => Some(config.clone()),
                _ => None,
            })
            .unwrap_or_default()
    });
    let bad_field = current
        .read()
        .check()
        .and_then(|(code, _)| field_of_code(code));
    let flagged =
        move |field: &'static str| bad_field == Some(field) || parse_errors.read().contains(&field);
    let (s_lo, s_hi) = pnex_core::VIDEO_SEGMENT_SECS_RANGE;
    let (m_lo, m_hi) = pnex_core::VIDEO_SEGMENT_MB_RANGE;
    let (g_lo, g_hi) = pnex_core::VIDEO_GAP_SECS_RANGE;
    let r_max = pnex_core::VIDEO_RETENTION_DAYS_MAX;

    rsx! {
        div { class: "space-y-3",
            p { class: "text-xs text-gray-500", {t!("flows-video-record-help")} }
            {field_input(t!("flows-video-segment-secs"), t!("flows-video-segment-secs-hint", min: s_lo, max: s_hi),
                segment_secs, true, flagged(F_SEGMENT_SECS), !can_write, move |event| {
                    let mut sig = segment_secs;
                    sig.set(event.value());
                    apply_field(&mut cx, F_SEGMENT_SECS, event.value(), parse_errors);
                })}
            {field_input(t!("flows-video-segment-mb"), t!("flows-video-segment-mb-hint", min: m_lo, max: m_hi),
                segment_mb, true, flagged(F_SEGMENT_MB), !can_write, move |event| {
                    let mut sig = segment_mb;
                    sig.set(event.value());
                    apply_field(&mut cx, F_SEGMENT_MB, event.value(), parse_errors);
                })}
            {field_input(t!("flows-video-gap-secs"), t!("flows-video-gap-secs-hint", min: g_lo, max: g_hi),
                gap_secs, true, flagged(F_GAP_SECS), !can_write, move |event| {
                    let mut sig = gap_secs;
                    sig.set(event.value());
                    apply_field(&mut cx, F_GAP_SECS, event.value(), parse_errors);
                })}
            {field_input(t!("flows-camera-max-fps"), t!("flows-camera-max-fps-hint"),
                max_fps, true, flagged(F_MAX_FPS), !can_write, move |event| {
                    let mut sig = max_fps;
                    sig.set(event.value());
                    apply_field(&mut cx, F_MAX_FPS, event.value(), parse_errors);
                })}
            {field_input(t!("flows-video-retention"), t!("flows-video-retention-hint", max: r_max),
                retention, true, flagged(F_RETENTION), !can_write, move |event| {
                    let mut sig = retention;
                    sig.set(event.value());
                    apply_field(&mut cx, F_RETENTION, event.value(), parse_errors);
                })}
            {field_input(t!("flows-video-stream"), t!("flows-video-stream-hint"),
                stream, false, flagged(F_STREAM), !can_write, move |event| {
                    let mut sig = stream;
                    sig.set(event.value());
                    apply_field(&mut cx, F_STREAM, event.value(), parse_errors);
                })}
        }
    }
}
