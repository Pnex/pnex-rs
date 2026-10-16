//! Frames of a camera recording added to the project (D202): camera, local
//! day and times, one picture every N seconds — one import, one source.

use chrono::TimeZone as _;
use dioxus::prelude::*;
use dioxus_i18n::t;
use pnex_core::lab::{LabRecordingImport, LAB_RECORDING_MAX_FRAMES, LAB_RECORDING_MAX_SECS};

use crate::api;
use crate::components::modal::Modal;
use crate::components::ui::{BTN_PRIMARY, BTN_SECONDARY, FIELD_LABEL, INPUT};
use crate::state::toasts;

/// RFC 3339 of a local day + `HH:MM` (None when unparsable or ambiguous).
fn local_rfc3339(day: &str, time: &str) -> Option<(String, i64)> {
    let date = chrono::NaiveDate::parse_from_str(day, "%Y-%m-%d").ok()?;
    let t = chrono::NaiveTime::parse_from_str(time, "%H:%M").ok()?;
    let at = chrono::Local
        .from_local_datetime(&date.and_time(t))
        .earliest()?;
    Some((at.to_rfc3339(), at.timestamp_millis()))
}

/// Pictures an extract would give (upper bound) and whether it is allowed.
fn estimate(from_ms: i64, to_ms: i64, every: f64) -> Option<usize> {
    let span = to_ms - from_ms;
    if span <= 0 || span > LAB_RECORDING_MAX_SECS * 1000 || !(0.5..=600.0).contains(&every) {
        return None;
    }
    let n = (span as f64 / (every * 1000.0)).ceil() as usize;
    (n <= LAB_RECORDING_MAX_FRAMES).then_some(n)
}

#[component]
pub(super) fn RecordingImport(
    project: String,
    on_close: EventHandler<()>,
    on_done: EventHandler<()>,
) -> Element {
    let cameras = use_resource(|| async { api::cameras::list().await.unwrap_or_default() });
    let now = chrono::Local::now();
    let mut device = use_signal(|| None::<i64>);
    let mut day = use_signal(|| now.format("%Y-%m-%d").to_string());
    let mut start = use_signal(|| {
        (now - chrono::Duration::minutes(10))
            .format("%H:%M")
            .to_string()
    });
    let mut end = use_signal(|| now.format("%H:%M").to_string());
    let mut every = use_signal(|| "5".to_string());
    let mut busy = use_signal(|| false);

    let cams = cameras.read().clone().unwrap_or_default();
    let from = local_rfc3339(&day(), &start());
    let to = local_rfc3339(&day(), &end());
    let every_v = every()
        .trim()
        .replace(',', ".")
        .parse::<f64>()
        .unwrap_or(0.0);
    let count = match (&from, &to) {
        (Some((_, a)), Some((_, b))) => estimate(*a, *b, every_v),
        _ => None,
    };
    let hint = match count {
        Some(n) => t!("lab-rec-estimate", count : n).to_string(),
        None => t!("lab-rec-invalid", max : LAB_RECORDING_MAX_FRAMES, minutes : LAB_RECORDING_MAX_SECS / 60).to_string(),
    };
    let ok = device().is_some() && count.is_some() && !busy();

    let submit = move |_| {
        let (Some(device), Some((from, _)), Some((to, _))) = (device(), from.clone(), to.clone())
        else {
            return;
        };
        let req = LabRecordingImport {
            device,
            from,
            to,
            every_secs: every_v,
        };
        let pid = project.clone();
        busy.set(true);
        spawn(async move {
            match api::lab::import_recording(&pid, &req).await {
                Ok(list) => {
                    toasts::success(t!("lab-rec-done", count : list.len()).to_string());
                    on_done.call(());
                }
                Err(e) => toasts::error(e),
            }
            busy.set(false);
        });
    };

    rsx! {
        Modal {
            title: t!("lab-rec-title").to_string(),
            max_width: "max-w-lg".to_string(),
            on_close: move |_| on_close.call(()),
            div { class: "space-y-3",
                p { class: "text-xs text-slate", {t!("lab-rec-help")} }
                label { class: "block",
                    span { class: FIELD_LABEL, {t!("lab-rec-camera")} }
                    select {
                        class: "{INPUT} w-full",
                        onchange: move |e| device.set(e.value().parse().ok()),
                        option { value: "", selected: device().is_none(), {t!("lab-rec-camera-none")} }
                        for cam in cams {
                            option { key: "{cam.device}", value: "{cam.device}", "{cam.device_id}" }
                        }
                    }
                }
                div { class: "grid grid-cols-3 gap-2",
                    label { class: "block",
                        span { class: FIELD_LABEL, {t!("lab-rec-day")} }
                        input {
                            class: "{INPUT} w-full",
                            r#type: "date",
                            value: "{day}",
                            oninput: move |e| day.set(e.value()),
                        }
                    }
                    label { class: "block",
                        span { class: FIELD_LABEL, {t!("lab-rec-from")} }
                        input {
                            class: "{INPUT} w-full",
                            r#type: "time",
                            value: "{start}",
                            oninput: move |e| start.set(e.value()),
                        }
                    }
                    label { class: "block",
                        span { class: FIELD_LABEL, {t!("lab-rec-to")} }
                        input {
                            class: "{INPUT} w-full",
                            r#type: "time",
                            value: "{end}",
                            oninput: move |e| end.set(e.value()),
                        }
                    }
                }
                label { class: "block",
                    span { class: FIELD_LABEL, {t!("lab-rec-every")} }
                    input {
                        class: "{INPUT} w-full",
                        r#type: "number",
                        min: "0.5",
                        max: "600",
                        step: "0.5",
                        value: "{every}",
                        oninput: move |e| every.set(e.value()),
                    }
                }
                p { class: if count.is_some() { "text-xs text-slate" } else { "text-xs text-amber-700" },
                    "{hint}"
                }
                div { class: "flex justify-end gap-2 pt-2",
                    button {
                        class: BTN_SECONDARY,
                        r#type: "button",
                        onclick: move |_| on_close.call(()),
                        {t!("common-cancel")}
                    }
                    button {
                        class: BTN_PRIMARY,
                        r#type: "button",
                        disabled: !ok,
                        onclick: submit,
                        {t!("lab-rec-import")}
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
    fn estimate_bounds() {
        assert_eq!(estimate(0, 60_000, 5.0), Some(12));
        assert_eq!(estimate(0, 0, 5.0), None);
        assert_eq!(estimate(0, 3_600_000, 1.0), None, "over 200 pictures");
        assert!(local_rfc3339("2026-10-13", "08:30").is_some());
        assert!(local_rfc3339("nope", "08:30").is_none());
    }
}
