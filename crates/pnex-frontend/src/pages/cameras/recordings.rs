//! Continuous recording of one camera, one local day at a time: coverage
//! timeline (merged segment spans + seek slider), the chained player, a
//! one-file export of the playhead's hour and a day delete (owner/admin).
//! The per-minute segments stay a storage detail. Detection layers of the
//! day (D105) are ticked to draw their boxes over the playback.

use chrono::TimeZone as _;
use dioxus::prelude::*;
use dioxus_i18n::t;
use pnex_core::camera::{playback_chain, CameraView, VideoSegment};

use super::player::{layer_color, span_ms, RecordingPlayer};
use crate::api;
use crate::components::confirm::ConfirmDialog;
use crate::components::crud::layout::DANGER_BTN;
use crate::components::crud::states::ListStates;
use crate::components::icons;
use crate::components::modal::Modal;
use crate::state::toasts;
use crate::util::trigger_download;

/// Gap under which two consecutive segments draw as one covered block.
const MERGE_GAP_MS: i64 = 5_000;

/// Human-readable size (B/KB/MB/GB).
fn format_size(bytes: i64) -> String {
    const UNITS: [&str; 4] = ["B", "KB", "MB", "GB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} B")
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

/// `3725` → `1 h 02 min`, `125` → `2 min` (data, no i18n: unit symbols).
fn format_recorded(secs: i64) -> String {
    let (h, m) = (secs / 3600, (secs % 3600) / 60);
    if h > 0 {
        format!("{h} h {m:02} min")
    } else if m > 0 {
        format!("{m} min")
    } else {
        format!("{secs} s")
    }
}

/// Local day `[00:00, next day 00:00)` as unix ms.
fn day_bounds(date: chrono::NaiveDate) -> Option<(i64, i64)> {
    let start = chrono::Local
        .from_local_datetime(&date.and_hms_opt(0, 0, 0)?)
        .earliest()?;
    let end = chrono::Local
        .from_local_datetime(&date.succ_opt()?.and_hms_opt(0, 0, 0)?)
        .earliest()?;
    Some((start.timestamp_millis(), end.timestamp_millis()))
}

/// Unix ms → RFC 3339 (UTC).
fn rfc3339(ms: i64) -> String {
    chrono::DateTime::from_timestamp_millis(ms)
        .map(|t| t.to_rfc3339())
        .unwrap_or_default()
}

/// Local-hour window `[HH:00, HH+1:00)` containing `ms`.
fn hour_window(ms: i64) -> Option<(i64, i64)> {
    let t = chrono::DateTime::from_timestamp_millis(ms)?.with_timezone(&chrono::Local);
    let start = t
        .date_naive()
        .and_hms_opt(t.format("%H").to_string().parse().ok()?, 0, 0)?;
    let start = chrono::Local.from_local_datetime(&start).earliest()?;
    let start_ms = start.timestamp_millis();
    Some((start_ms, start_ms + 3_600_000))
}

/// Merges spans closer than `gap` into covered blocks (timeline drawing).
fn coverage(spans: &[(i64, i64)], gap: i64) -> Vec<(i64, i64)> {
    let mut out: Vec<(i64, i64)> = Vec::new();
    for &(s, e) in spans {
        match out.last_mut() {
            Some(last) if s - last.1 <= gap => last.1 = last.1.max(e),
            _ => out.push((s, e)),
        }
    }
    out
}

/// Local `HH:MM` of unix ms.
fn hhmm(ms: i64) -> String {
    chrono::DateTime::from_timestamp_millis(ms)
        .map(|t| t.with_timezone(&chrono::Local).format("%H:%M").to_string())
        .unwrap_or_default()
}

/// Download name: `<camera>_<YYYYmmdd-HHMM>.avi` (slug-safe).
fn export_name(camera: &str, from_ms: i64) -> String {
    let stamp = chrono::DateTime::from_timestamp_millis(from_ms)
        .map(|t| {
            t.with_timezone(&chrono::Local)
                .format("%Y%m%d-%H%M")
                .to_string()
        })
        .unwrap_or_default();
    let camera: String = camera
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect();
    format!("{camera}_{stamp}.avi")
}

#[component]
pub(super) fn RecordingsDialog(
    cam: CameraView,
    can_write: bool,
    on_close: Callback<()>,
) -> Element {
    let device = cam.device;
    let mut day = use_signal(|| chrono::Local::now().date_naive());
    let mut reload = use_signal(|| 0u32);
    let mut seek = use_signal(|| None::<i64>);
    let mut position = use_signal(|| None::<i64>);
    let mut exporting = use_signal(|| false);
    let mut confirm_delete = use_signal(|| false);
    // Ticked layer ids, in tick order (kept across days: ids are stable).
    let mut layers = use_signal(Vec::<String>::new);

    let timeline = use_resource(move || {
        let bounds = day_bounds(day());
        async move {
            let _ = reload();
            let (from, to) = bounds.unwrap_or((0, 1));
            api::cameras::recordings(device, &rfc3339(from), &rfc3339(to)).await
        }
    });

    let available_layers = use_resource(move || {
        let bounds = day_bounds(day());
        async move {
            let _ = reload();
            let (from, to) = bounds.unwrap_or((0, 1));
            api::cameras::annotation_layers(device, &rfc3339(from), &rfc3339(to))
                .await
                .unwrap_or_default()
        }
    });
    let layer_rows = available_layers.value().read().clone().unwrap_or_default();

    let (day_start, day_end) = day_bounds(day()).unwrap_or((0, 86_400_000));
    let day_len = (day_end - day_start).max(1);
    let (state, chain, total_bytes, truncated) = match &*timeline.value().read() {
        None => (None, Vec::new(), 0, false),
        Some(Ok(tl)) => {
            let spans: Vec<(i64, i64)> = tl
                .segments
                .iter()
                .map(|s| span_ms(s).unwrap_or((0, 0)))
                .collect();
            let chain: Vec<VideoSegment> = playback_chain(&spans)
                .into_iter()
                .map(|i| tl.segments[i].clone())
                .collect();
            (Some(Ok(())), chain, tl.total_bytes, tl.truncated)
        }
        Some(Err(err)) => (Some(Err(err.clone())), Vec::new(), 0, false),
    };
    let chain_spans: Vec<(i64, i64)> = chain.iter().filter_map(span_ms).collect();
    let blocks = coverage(&chain_spans, MERGE_GAP_MS);
    let recorded_secs: i64 = chain_spans.iter().map(|(s, e)| (e - s) / 1000).sum();
    let is_empty = chain.is_empty();
    let pct = move |ms: i64| ((ms - day_start) as f64 / day_len as f64 * 100.0).clamp(0.0, 100.0);
    let cursor = position().filter(|&p| p >= day_start && p < day_end);
    let slider_secs = cursor.map(|p| (p - day_start) / 1000).unwrap_or(0);
    let export_window = cursor.and_then(hour_window);
    let today = chrono::Local::now().date_naive();
    let player_key = format!("player-{device}-{}-{}", day(), reload());
    let camera_name = cam.device_id.clone();

    rsx! {
        Modal {
            title: t!("cameras-recordings-title", camera : cam.device_id.clone()).to_string(),
            max_width: "max-w-5xl".to_string(),
            on_close,
            div { class: "space-y-4",
                // Day navigation + summary.
                div { class: "flex flex-wrap items-center gap-2",
                    button {
                        class: "px-2 py-2 text-gray-700 border border-gray-300 rounded-lg hover:bg-gray-50",
                        r#type: "button",
                        title: t!("cameras-day-prev"),
                        onclick: move |_| {
                            if let Some(d) = day().pred_opt() {
                                position.set(None);
                                day.set(d);
                            }
                        },
                        icons::ChevronLeft { class: "h-4 w-4" }
                    }
                    input {
                        class: "px-3 py-2 border border-gray-300 rounded-lg text-sm",
                        r#type: "date",
                        value: "{day().format(\"%Y-%m-%d\")}",
                        max: "{today.format(\"%Y-%m-%d\")}",
                        onchange: move |event| {
                            if let Ok(d) = chrono::NaiveDate::parse_from_str(&event.value(), "%Y-%m-%d") {
                                position.set(None);
                                day.set(d);
                            }
                        },
                    }
                    button {
                        class: "px-2 py-2 text-gray-700 border border-gray-300 rounded-lg hover:bg-gray-50 disabled:opacity-40",
                        r#type: "button",
                        disabled: day() >= today,
                        title: t!("cameras-day-next"),
                        onclick: move |_| {
                            if let Some(d) = day().succ_opt() {
                                position.set(None);
                                day.set(d);
                            }
                        },
                        icons::ChevronRight { class: "h-4 w-4" }
                    }
                    if !is_empty {
                        span { class: "text-sm text-gray-500",
                            {
                                t!(
                                    "cameras-recorded-summary", duration : format_recorded(recorded_secs), size :
                                    format_size(total_bytes)
                                )
                            }
                        }
                    }
                    div { class: "ml-auto flex gap-2",
                        button {
                            class: "inline-flex items-center px-3 py-1.5 text-sm text-gray-700 border border-gray-300 rounded-lg hover:bg-gray-50 disabled:opacity-40",
                            r#type: "button",
                            disabled: exporting() || export_window.is_none(),
                            title: t!("cameras-export-hint"),
                            onclick: move |_| {
                                let Some((from, to)) = export_window else {
                                    return;
                                };
                                let name = export_name(&camera_name, from);
                                let failed = t!("cameras-download-failed").to_string();
                                spawn(async move {
                                    exporting.set(true);
                                    match api::cameras::export_recording(device, &rfc3339(from), &rfc3339(to))
                                        .await
                                    {
                                        Ok(bytes) => {
                                            if !trigger_download(&name, "video/x-msvideo", &bytes) {
                                                toasts::error(failed);
                                            }
                                        }
                                        Err(err) => {
                                            toasts::error(
                                                format!("{failed} : {}", crate::api::error_i18n::localize(&err)),
                                            )
                                        }
                                    }
                                    exporting.set(false);
                                });
                            },
                            icons::Download { class: "h-4 w-4 mr-1" }
                            match export_window {
                                Some((from, to)) => rsx! {
                                    {t!("cameras-export-hour", from : hhmm(from), to : hhmm(to))}
                                },
                                None => rsx! {
                                    {t!("cameras-download")}
                                },
                            }
                        }
                        if can_write && !is_empty {
                            button {
                                class: DANGER_BTN,
                                r#type: "button",
                                onclick: move |_| confirm_delete.set(true),
                                {t!("cameras-delete-day")}
                            }
                        }
                    }
                }
                ListStates {
                    state,
                    is_empty,
                    empty_message: t!("cameras-recordings-empty-title").to_string(),
                    empty_icon: rsx! {
                        icons::Video { class: "h-8 w-8 text-gray-400" }
                    },
                    empty_detail: rsx! {
                        p { class: "text-gray-600 mt-2 max-w-xl mx-auto", {t!("cameras-recordings-empty-message")} }
                    },
                    div { class: "space-y-3",
                        if layer_rows.is_empty() {
                            p { class: "text-xs text-gray-400", {t!("cameras-layers-none")} }
                        } else {
                            div { class: "flex flex-wrap items-center gap-x-4 gap-y-1",
                                span { class: "text-xs font-medium text-gray-500",
                                    {t!("cameras-layers")}
                                }
                                for layer in layer_rows.iter().cloned() {
                                    label {
                                        key: "{layer.id}",
                                        class: "flex items-center gap-1.5 text-sm text-gray-700 select-none",
                                        input {
                                            class: "h-4 w-4 accent-blue-600",
                                            r#type: "checkbox",
                                            checked: layers.read().contains(&layer.id),
                                            onchange: {
                                                let id = layer.id.clone();
                                                move |event: FormEvent| {
                                                    let on = event.checked();
                                                    let mut next = layers.peek().clone();
                                                    next.retain(|l| *l != id);
                                                    if on {
                                                        next.push(id.clone());
                                                    }
                                                    layers.set(next);
                                                }
                                            },
                                        }
                                        if let Some(rank) = layers.read().iter().position(|l| *l == layer.id) {
                                            span {
                                                class: "inline-block h-2.5 w-2.5 rounded-full",
                                                style: "background: {layer_color(rank)};",
                                            }
                                        }
                                        "{layer.name}"
                                        span { class: "text-xs text-gray-400", "({layer.count})" }
                                    }
                                }
                            }
                        }
                        // Own block: rsx only honours a key on a block's first node, and the
                        // remount on day/reload change drops the per-index segment cache.
                        {
                            rsx! {
                                RecordingPlayer {
                                    key: "{player_key}",
                                    device,
                                    segments: chain.clone(),
                                    layers,
                                    seek,
                                    position,
                                }
                            }
                        }
                        // Timeline: covered blocks behind a seek slider.
                        div { class: "space-y-1",
                            div { class: "relative h-3 rounded bg-gray-100 overflow-hidden",
                                for (s, e) in blocks.iter().copied() {
                                    div {
                                        key: "{s}",
                                        class: "absolute inset-y-0 bg-blue-400",
                                        style: "left: {pct(s)}%; width: max(2px, {pct(e) - pct(s)}%);",
                                    }
                                }
                                if let Some(p) = cursor {
                                    div {
                                        class: "absolute inset-y-0 w-0.5 bg-red-600",
                                        style: "left: {pct(p)}%;",
                                    }
                                }
                            }
                            input {
                                class: "w-full accent-blue-600",
                                r#type: "range",
                                min: "0",
                                max: "{day_len / 1000}",
                                value: "{slider_secs}",
                                onchange: move |event| {
                                    if let Ok(v) = event.value().parse::<i64>() {
                                        seek.set(Some(day_start + v * 1000));
                                    }
                                },
                            }
                            div { class: "flex justify-between text-[10px] text-gray-400 tabular-nums",
                                for h in (0..=24).step_by(3) {
                                    span { key: "{h}", "{h:02}:00" }
                                }
                            }
                            if truncated {
                                p { class: "text-xs text-amber-700",
                                    {t!("cameras-timeline-truncated")}
                                }
                            }
                        }
                    }
                }
            }
        }
        if confirm_delete() {
            ConfirmDialog {
                title: t!("cameras-delete-day-title"),
                message: t!(
                    "cameras-delete-day-message", camera : cam.device_id.clone(), day : day()
                    .format("%Y-%m-%d").to_string()
                ),
                confirm_label: t!("cameras-delete-confirm"),
                on_confirm: move |_| {
                    confirm_delete.set(false);
                    let done = t!("cameras-deleted").to_string();
                    let failed = t!("cameras-delete-failed").to_string();
                    spawn(async move {
                        match api::cameras::delete_recordings(
                                device,
                                &rfc3339(day_start),
                                &rfc3339(day_end),
                            )
                            .await
                        {
                            Ok(_) => {
                                toasts::success(done);
                                position.set(None);
                                reload.with_mut(|r| *r += 1);
                            }
                            Err(err) => {
                                toasts::error(
                                    format!("{failed} : {}", crate::api::error_i18n::localize(&err)),
                                )
                            }
                        }
                    });
                },
                on_cancel: move |_| confirm_delete.set(false),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn day_bounds_span_one_local_day() {
        let date = chrono::NaiveDate::from_ymd_opt(2026, 9, 29).unwrap();
        let (from, to) = day_bounds(date).unwrap();
        // 23 or 25 h on DST switch days, 24 otherwise.
        assert!((23..=25).contains(&((to - from) / 3_600_000)));
    }

    #[test]
    fn coverage_merges_close_spans() {
        let spans = [(0, 60_000), (60_200, 120_000), (200_000, 260_000)];
        assert_eq!(
            coverage(&spans, 5_000),
            vec![(0, 120_000), (200_000, 260_000)]
        );
        assert!(coverage(&[], 5_000).is_empty());
    }

    #[test]
    fn hour_window_and_labels() {
        let now = chrono::Local::now().timestamp_millis();
        let (from, to) = hour_window(now).unwrap();
        assert!(from <= now && now < to);
        assert_eq!(to - from, 3_600_000);
        assert_eq!(format_recorded(3725), "1 h 02 min");
        assert_eq!(format_recorded(125), "2 min");
        assert_eq!(format_size(2 * 1024 * 1024), "2.0 MB");
        assert!(export_name("proud robin", now).starts_with("proud_robin_"));
    }
}
