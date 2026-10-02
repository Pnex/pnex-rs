//! "Test with an image" modal: pick a JPEG/PNG, POST it to
//! `/api/v1/ml/models/{id}/test`, draw the detections (SVG overlay in the
//! source image pixel space) with labels, scores and inference time.

use dioxus::prelude::*;
use dioxus_i18n::t;
use pnex_core::vision::{DetectionResult, MlModel};

use crate::api;
use crate::api::error::ApiError;
use crate::components::modal::Modal;
use crate::util::{image_url_from_bytes, release_image_url};

/// Stroke colors cycled per class id (full hex literals).
const BOX_COLORS: [&str; 6] = [
    "#d946ef", "#22c55e", "#3b82f6", "#f59e0b", "#ef4444", "#06b6d4",
];

/// Box color of a class id.
pub(super) fn box_color(class_id: u32) -> &'static str {
    BOX_COLORS[class_id as usize % BOX_COLORS.len()]
}

/// MIME type from the picked file name (the test endpoint sniffs the bytes;
/// this only feeds the native data URI).
fn mime_of(name: &str) -> &'static str {
    if name.to_ascii_lowercase().ends_with(".png") {
        "image/png"
    } else {
        "image/jpeg"
    }
}

#[component]
pub(super) fn TestImageModal(model: MlModel, on_close: Callback<()>) -> Element {
    let mut image_url = use_signal(|| None::<String>);
    let mut result = use_signal(|| None::<DetectionResult>);
    let mut error = use_signal(|| None::<ApiError>);
    let mut running = use_signal(|| false);
    let model_id = model.id.clone();

    use_drop(move || {
        if let Some(url) = image_url.peek().clone() {
            release_image_url(&url);
        }
    });

    let res = result();
    let stroke = res
        .as_ref()
        .map(|r| (r.width.max(r.height) as f32 / 300.0).max(1.0))
        .unwrap_or(2.0);
    let font = stroke * 7.0;

    rsx! {
        Modal {
            title: t!("models-test-title", name : model.name.clone()),
            max_width: "max-w-4xl".to_string(),
            on_close: move |_| on_close.call(()),
            div { class: "space-y-4",
                input {
                    class: "w-full text-sm border border-gray-300 rounded-lg px-3 py-2",
                    r#type: "file",
                    accept: "image/jpeg,image/png",
                    onchange: move |evt| {
                        let Some(file) = evt.files().first().cloned() else {
                            return;
                        };
                        let id = model_id.clone();
                        let mime = mime_of(&file.name());
                        spawn(async move {
                            let Ok(bytes) = file.read_bytes().await else {
                                return;
                            };
                            let bytes = bytes.to_vec();
                            if let Some(prev) = image_url.peek().clone() {
                                release_image_url(&prev);
                            }
                            image_url.set(image_url_from_bytes(&bytes, mime));
                            result.set(None);
                            error.set(None);
                            running.set(true);
                            match api::ml_models::test_image(&id, bytes).await {
                                Ok(r) => result.set(Some(r)),
                                Err(e) => error.set(Some(e)),
                            }
                            running.set(false);
                        });
                    },
                }
                p { class: "text-xs text-gray-500", {t!("models-test-hint")} }
                if running() {
                    div { class: "flex items-center gap-2 text-sm text-gray-600",
                        span { class: "animate-spin inline-block rounded-full h-4 w-4 border-b-2 border-blue-600" }
                        {t!("models-test-running")}
                    }
                }
                if let Some(e) = error() {
                    div { class: "rounded-lg bg-red-50 border border-red-200 p-2 text-sm text-red-700",
                        {crate::api::error_i18n::localize(&e)}
                    }
                }
                if let Some(url) = image_url() {
                    div { class: "relative w-full bg-gray-900 rounded-lg overflow-hidden",
                        img {
                            class: "block w-full h-auto",
                            src: "{url}",
                            alt: "",
                        }
                        if let Some(r) = res.clone() {
                            svg {
                                class: "absolute inset-0 w-full h-full pointer-events-none",
                                view_box: "0 0 {r.width} {r.height}",
                                preserve_aspect_ratio: "none",
                                for (i, d) in r.detections.iter().enumerate() {
                                    g { key: "{i}",
                                        rect {
                                            x: "{d.bbox[0]}",
                                            y: "{d.bbox[1]}",
                                            width: "{d.bbox[2]}",
                                            height: "{d.bbox[3]}",
                                            fill: "none",
                                            stroke: box_color(d.class_id),
                                            stroke_width: "{stroke}",
                                        }
                                        rect {
                                            x: "{d.bbox[0]}",
                                            y: "{(d.bbox[1] - font * 1.4).max(0.0)}",
                                            width: "{font * 0.62 * (d.label.chars().count() as f32 + 5.0)}",
                                            height: "{font * 1.4}",
                                            fill: box_color(d.class_id),
                                        }
                                        text {
                                            x: "{d.bbox[0] + font * 0.3}",
                                            y: "{(d.bbox[1] - font * 1.4).max(0.0) + font * 1.05}",
                                            fill: "#ffffff",
                                            font_size: "{font}",
                                            font_family: "sans-serif",
                                            "{d.label} {d.score:.2}"
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
                if let Some(r) = res {
                    div { class: "space-y-2",
                        p { class: "text-sm text-gray-700",
                            {
                                t!(
                                    "models-test-summary", count : r.detections.len(), ms : r.took_ms, width : r
                                    .width, height : r.height
                                )
                            }
                        }
                        if r.detections.is_empty() {
                            p { class: "text-sm text-gray-500", {t!("models-test-none")} }
                        } else {
                            div { class: "flex flex-wrap gap-2",
                                for (i, d) in r.detections.iter().enumerate() {
                                    span {
                                        key: "{i}",
                                        class: "px-2 py-0.5 rounded-full text-xs text-white",
                                        style: "background-color: {box_color(d.class_id)}",
                                        "{d.label} · {d.score:.2}"
                                    }
                                }
                            }
                        }
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
    fn box_colors_cycle() {
        assert_eq!(box_color(0), box_color(6));
        assert_ne!(box_color(0), box_color(1));
        assert_eq!(mime_of("a.PNG"), "image/png");
        assert_eq!(mime_of("dog.jpg"), "image/jpeg");
    }
}
