//! "Live test" modal (camera-video.md D104): pick a camera, loop
//! `POST /api/v1/ml/models/{id}/test-live` (next request once the previous
//! answer is drawn, at least [`ROUND_GAP_MS`] apart) and draw every
//! detection on the analysed frame — solid above the threshold, dashed grey
//! below — with the camera state and the server warnings. The threshold
//! slider is local; owners/admins can apply it to the model.

use std::time::Duration;

use base64::Engine as _;
use dioxus::prelude::*;
use dioxus_i18n::t;
use pnex_core::camera::CameraView;
use pnex_core::vision::{LiveTestResult, MlModel};

use crate::api;
use crate::api::error::ApiError;
use crate::components::crud::layout::{GHOST_BTN, PRIMARY_BTN};
use crate::components::modal::Modal;
use crate::state::toasts;
use crate::util::{image_url_from_bytes, release_image_url, sleep};

use super::test_image::box_color;

/// Minimum spacing between two live rounds.
const ROUND_GAP_MS: u64 = 500;
/// Stroke of below-threshold boxes.
const BELOW_COLOR: &str = "#9ca3af";

#[component]
pub(super) fn TestLiveModal(
    model: MlModel,
    can_write: bool,
    on_close: Callback<()>,
    on_threshold_saved: Callback<()>,
) -> Element {
    let mut device = use_signal(|| None::<i64>);
    let mut running = use_signal(|| false);
    // Bumped on every start/stop: a round of an older loop never writes.
    let mut generation = use_signal(|| 0u64);
    let mut last = use_signal(|| None::<LiveTestResult>);
    let mut error = use_signal(|| None::<ApiError>);
    let mut image_url = use_signal(|| None::<String>);
    let threshold = use_signal(|| model.spec.score_threshold);
    let mut saving = use_signal(|| false);
    let model_id = model.id.clone();

    let cameras = use_resource(|| async { api::cameras::list().await.unwrap_or_default() });
    let camera_rows: Vec<CameraView> = cameras.value().read().clone().unwrap_or_default();
    // Preselect the first camera: a select showing a value the signal does
    // not hold would start nothing.
    if device.peek().is_none() {
        if let Some(first) = camera_rows.first() {
            device.set(Some(first.device));
        }
    }

    use_drop(move || {
        if let Some(url) = image_url.peek().clone() {
            release_image_url(&url);
        }
    });

    let start = move |_| {
        let Some(dev) = device() else {
            return;
        };
        let id = model_id.clone();
        generation.with_mut(|g| *g += 1);
        let mine = *generation.peek();
        running.set(true);
        error.set(None);
        spawn(async move {
            while *running.peek() && *generation.peek() == mine {
                let res = api::ml_models::test_live(&id, dev).await;
                if *generation.peek() != mine {
                    break;
                }
                match res {
                    Ok(r) => {
                        let bytes = base64::engine::general_purpose::STANDARD
                            .decode(r.frame_b64.as_bytes())
                            .unwrap_or_default();
                        if let Some(prev) = image_url.peek().clone() {
                            release_image_url(&prev);
                        }
                        image_url.set(image_url_from_bytes(&bytes, "image/jpeg"));
                        error.set(None);
                        last.set(Some(r));
                    }
                    Err(e) => {
                        // Offline camera, no frame, invalid model: stop and
                        // show why instead of hammering the server.
                        error.set(Some(e));
                        running.set(false);
                        break;
                    }
                }
                sleep(Duration::from_millis(ROUND_GAP_MS)).await;
            }
        });
    };
    let stop = move |_| {
        running.set(false);
        generation.with_mut(|g| *g += 1);
    };

    let apply_threshold = {
        let id = model.id.clone();
        let spec = model.spec.clone();
        move |_| {
            let id = id.clone();
            let mut spec = spec.clone();
            spec.score_threshold = (threshold() * 100.0).round() / 100.0;
            let done = t!("models-live-threshold-saved").to_string();
            spawn(async move {
                saving.set(true);
                let input = api::ml_models::UpdateModel {
                    spec: Some(spec),
                    ..Default::default()
                };
                match api::ml_models::update(&id, &input).await {
                    Ok(_) => {
                        toasts::success(done);
                        on_threshold_saved.call(());
                    }
                    Err(e) => error.set(Some(e)),
                }
                saving.set(false);
            });
        }
    };

    let thr = threshold();
    let res = last();
    let (stroke, font) = res
        .as_ref()
        .map(|r| {
            let s = (r.result.width.max(r.result.height) as f32 / 300.0).max(1.0);
            (s, s * 7.0)
        })
        .unwrap_or((1.0, 7.0));
    // Sorted by score, highest first, split around the local threshold.
    let mut dets = res
        .as_ref()
        .map(|r| r.result.detections.clone())
        .unwrap_or_default();
    dets.sort_by(|a, b| b.score.total_cmp(&a.score));

    rsx! {
        Modal {
            title: t!("models-live-title", name : model.name.clone()),
            max_width: "max-w-4xl".to_string(),
            on_close: move |_| on_close.call(()),
            div { class: "space-y-4",
                if camera_rows.is_empty() && cameras.value().read().is_some() {
                    p { class: "text-sm text-gray-600", {t!("models-live-no-camera")} }
                } else {
                    div { class: "flex flex-wrap items-end gap-3",
                        div { class: "flex-1 min-w-48",
                            label {
                                r#for: "test-live-field-1",
                                class: "block text-sm font-medium text-gray-700 mb-1",
                                {t!("models-live-camera")}
                            }
                            select {
                                id: "test-live-field-1",
                                class: "w-full px-3 py-2 border border-gray-300 rounded-lg text-sm bg-white",
                                disabled: running(),
                                onchange: move |e| device.set(e.value().parse::<i64>().ok()),
                                for cam in camera_rows.clone() {
                                    option {
                                        key: "{cam.device}",
                                        value: "{cam.device}",
                                        selected: device() == Some(cam.device),
                                        "{cam.device_id}"
                                    }
                                }
                            }
                        }
                        if running() {
                            button {
                                class: GHOST_BTN,
                                r#type: "button",
                                onclick: stop,
                                {t!("models-live-stop")}
                            }
                        } else {
                            button {
                                class: PRIMARY_BTN,
                                r#type: "button",
                                disabled: device().is_none(),
                                onclick: start,
                                {t!("models-live-start")}
                            }
                        }
                    }
                }
                p { class: "text-xs text-gray-500", {t!("models-live-hint")} }
                LiveThreshold {
                    threshold,
                    model_threshold: model.spec.score_threshold,
                    can_write,
                    saving: saving(),
                    on_apply: apply_threshold,
                }
                if let Some(e) = error() {
                    div { class: "rounded-lg bg-red-50 border border-red-200 p-2 text-sm text-red-700",
                        {crate::api::error_i18n::localize(&e)}
                    }
                }
                if let Some(r) = res.clone() {
                    LiveInfo { result: r }
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
                                view_box: "0 0 {r.result.width} {r.result.height}",
                                preserve_aspect_ratio: "none",
                                for (i, d) in dets.iter().enumerate() {
                                    g { key: "{i}",
                                        rect {
                                            x: "{d.bbox[0]}",
                                            y: "{d.bbox[1]}",
                                            width: "{d.bbox[2]}",
                                            height: "{d.bbox[3]}",
                                            fill: "none",
                                            stroke: if d.score >= thr { box_color(d.class_id) } else { BELOW_COLOR },
                                            stroke_width: "{stroke}",
                                            stroke_dasharray: if d.score >= thr { "none".to_string() } else { format!("{} {}", stroke * 3.0, stroke * 2.0) },
                                        }
                                        text {
                                            x: "{d.bbox[0] + font * 0.3}",
                                            y: "{(d.bbox[1] + font * 1.1).min(r.result.height as f32)}",
                                            fill: if d.score >= thr { box_color(d.class_id) } else { BELOW_COLOR },
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
                if res.is_some() {
                    if dets.is_empty() {
                        p { class: "text-sm text-gray-500", {t!("models-live-none")} }
                    } else {
                        div { class: "flex flex-wrap gap-2",
                            for (i, d) in dets.iter().enumerate() {
                                if d.score >= thr {
                                    span {
                                        key: "{i}",
                                        class: "px-2 py-0.5 rounded-full text-xs text-white",
                                        style: "background-color: {box_color(d.class_id)}",
                                        "{d.label} · {d.score:.2}"
                                    }
                                } else {
                                    span {
                                        key: "{i}",
                                        class: "px-2 py-0.5 rounded-full text-xs text-gray-600 border border-dashed border-gray-400",
                                        "{d.label} · {d.score:.2} · "
                                        {t!("models-live-below")}
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

/// Local threshold slider (+ apply to the model for owners/admins).
#[component]
fn LiveThreshold(
    threshold: Signal<f32>,
    model_threshold: f32,
    can_write: bool,
    saving: bool,
    on_apply: EventHandler<()>,
) -> Element {
    let value = threshold();
    let changed = (value - model_threshold).abs() > 0.004;
    rsx! {
        div { class: "flex flex-wrap items-center gap-3",
            label {
                r#for: "test-live-field-2",
                class: "text-sm text-gray-700 whitespace-nowrap",
                {t!("models-live-threshold", value : format!("{value:.2}"))}
            }
            input {
                id: "test-live-field-2",
                class: "flex-1 min-w-40",
                r#type: "range",
                min: "0.05",
                max: "0.95",
                step: "0.01",
                value: "{value}",
                oninput: move |e| {
                    if let Ok(v) = e.value().parse::<f32>() {
                        threshold.set(v);
                    }
                },
            }
            if can_write && changed {
                button {
                    class: GHOST_BTN,
                    r#type: "button",
                    disabled: saving,
                    onclick: move |_| on_apply.call(()),
                    {t!("models-live-apply-threshold")}
                }
            }
        }
    }
}

/// Frame, inference and camera figures + server warnings.
#[component]
fn LiveInfo(result: LiveTestResult) -> Element {
    let r = result;
    rsx! {
        div { class: "space-y-1",
            p { class: "text-xs text-gray-600",
                {
                    t!(
                        "models-live-stats", seq : r.frame_seq, age : r.frame_age_ms, ms : r.result
                        .took_ms, width : r.result.width, height : r.result.height
                    )
                }
                " · "
                {
                    t!(
                        "models-live-camera-state", mode : r.camera.capture_mode.clone(), framesize :
                        r.camera.framesize.clone(), fps : r.camera.fps
                    )
                }
            }
            for w in r.warnings.iter() {
                p {
                    key: "{w}",
                    class: "text-xs text-amber-800 bg-amber-50 border border-amber-200 rounded px-2 py-1",
                    {warning_text(w)}
                }
            }
        }
    }
}

/// Live-test warning code → localized text (unknown code: verbatim).
fn warning_text(code: &str) -> String {
    match code {
        "camera-frame-stale" => t!("models-live-warn-camera-frame-stale").to_string(),
        "image-too-dark" => t!("models-live-warn-image-too-dark").to_string(),
        "inference-slower-than-camera" => {
            t!("models-live-warn-inference-slower-than-camera").to_string()
        }
        other => other.to_string(),
    }
}
