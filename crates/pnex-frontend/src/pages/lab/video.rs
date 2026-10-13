//! Frames of a video of the media library added to the project (MP4 /
//! MOV): video, start and end in seconds, one picture every N seconds.
//! Every import of the same video shares one source.

use dioxus::prelude::*;
use dioxus_i18n::t;
use pnex_core::lab::{LabVideoImport, LAB_RECORDING_MAX_FRAMES, LAB_RECORDING_MAX_SECS};

use crate::api;
use crate::api::media::{MediaFilters, MediaKind};
use crate::components::modal::Modal;
use crate::components::ui::{BTN_PRIMARY, BTN_SECONDARY, FIELD_LABEL, INPUT};
use crate::state::toasts;

/// `"1,5"` → 1.5; anything else unparsable → `None`.
fn secs(raw: &str) -> Option<f64> {
    raw.trim()
        .replace(',', ".")
        .parse::<f64>()
        .ok()
        .filter(|v| v.is_finite())
}

/// The request when it passes the shared core bounds, and its picture count.
fn request(asset_id: &str, from: &str, to: &str, every: &str) -> Option<(LabVideoImport, usize)> {
    let req = LabVideoImport {
        asset_id: asset_id.to_string(),
        from_secs: secs(from)?,
        to_secs: secs(to)?,
        every_secs: secs(every)?,
    };
    if asset_id.is_empty() || req.check().is_some() {
        return None;
    }
    let n = ((req.to_secs - req.from_secs) / req.every_secs).ceil() as usize;
    Some((req, n))
}

#[component]
pub(super) fn VideoImport(
    project: String,
    on_close: EventHandler<()>,
    on_done: EventHandler<()>,
) -> Element {
    let videos = use_resource(|| async {
        let filters = MediaFilters {
            kinds: vec![MediaKind::Video],
            limit: Some(100),
            ..Default::default()
        };
        api::media::list(&filters)
            .await
            .map(|p| p.results)
            .unwrap_or_default()
    });
    let mut asset = use_signal(String::new);
    let mut from = use_signal(|| "0".to_string());
    let mut to = use_signal(|| "60".to_string());
    let mut every = use_signal(|| "2".to_string());
    let mut busy = use_signal(|| false);

    let list = videos.read().clone().unwrap_or_default();
    let loaded = videos.read().is_some();
    let ready = request(&asset(), &from(), &to(), &every());
    let count = ready.as_ref().map(|r| r.1);
    let hint = match count {
        Some(n) => t!("lab-rec-estimate", count : n).to_string(),
        None => t!("lab-video-invalid", max : LAB_RECORDING_MAX_FRAMES, minutes : LAB_RECORDING_MAX_SECS / 60).to_string(),
    };
    let valid = ready.is_some();
    let ok = valid && !busy();

    let submit = move |_| {
        let Some((req, _)) = ready.clone() else {
            return;
        };
        let pid = project.clone();
        busy.set(true);
        spawn(async move {
            match api::lab::import_video(&pid, &req).await {
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
            title: t!("lab-video-title").to_string(),
            max_width: "max-w-lg".to_string(),
            on_close: move |_| on_close.call(()),
            div { class: "space-y-3",
                p { class: "text-xs text-gray-500", {t!("lab-video-help")} }
                label { class: "block",
                    span { class: FIELD_LABEL, {t!("lab-video-asset")} }
                    if loaded && list.is_empty() {
                        p { class: "text-xs text-amber-700", {t!("lab-video-none")} }
                    }
                    select {
                        class: "{INPUT} w-full",
                        onchange: move |e| asset.set(e.value()),
                        option { value: "", selected: asset().is_empty(), {t!("lab-video-pick")} }
                        for v in list {
                            option { key: "{v.id}", value: "{v.id}", "{v.name}" }
                        }
                    }
                }
                div { class: "grid grid-cols-3 gap-2",
                    label { class: "block",
                        span { class: FIELD_LABEL, {t!("lab-video-from")} }
                        input {
                            class: "{INPUT} w-full",
                            r#type: "number",
                            min: "0",
                            step: "1",
                            value: "{from}",
                            oninput: move |e| from.set(e.value()),
                        }
                    }
                    label { class: "block",
                        span { class: FIELD_LABEL, {t!("lab-video-to")} }
                        input {
                            class: "{INPUT} w-full",
                            r#type: "number",
                            min: "1",
                            step: "1",
                            value: "{to}",
                            oninput: move |e| to.set(e.value()),
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
                }
                p { class: if valid { "text-xs text-gray-600" } else { "text-xs text-amber-700" },
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
    fn request_uses_the_core_bounds() {
        assert_eq!(request("a", "0", "60", "2").map(|r| r.1), Some(30));
        assert_eq!(request("a", "0", "1,5", "0,5").map(|r| r.1), Some(3));
        assert!(request("", "0", "60", "2").is_none());
        assert!(request("a", "10", "5", "2").is_none());
        assert!(
            request("a", "0", "3600", "1").is_none(),
            "over 200 pictures"
        );
    }
}
