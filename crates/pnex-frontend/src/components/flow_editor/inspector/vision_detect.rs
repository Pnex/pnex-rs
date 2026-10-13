use super::helpers::*;
use super::*;

use crate::components::ui::FIELD_LABEL;
use pnex_core::vision::{VisionDetectConfig, VisionEmit};

// ─────────────── vision-detect (camera-video.md D83) ───────────────

/// Toggles `label` in the picked label list (order of the model kept by the
/// caller's chip iteration, not here).
fn toggle_label(labels: &mut Vec<String>, label: &str) {
    if let Some(pos) = labels.iter().position(|l| l == label) {
        labels.remove(pos);
    } else {
        labels.push(label.to_string());
    }
}

/// Vision detect inspector: model picker (`GET /api/v1/ml/models`), label
/// chips from the picked model's spec, min score (0 = model threshold),
/// emit mode and inference rate.
#[component]
pub(super) fn VisionDetectForm(
    mut cx: EditorCx,
    initial: VisionDetectConfig,
    can_write: bool,
) -> Element {
    let review_initial = initial.clone();
    let models = use_resource(move || async move {
        let page = api::ml_models::list(Some(100), None).await.ok()?;
        crate::state::vision::remember(&page.results);
        Some(page.results)
    });
    let mut model_id = use_signal(move || initial.model_id.clone());
    let mut labels = use_signal(move || initial.labels.clone());
    let mut score_raw = use_signal(move || initial.min_score.to_string());
    let mut score_invalid = use_signal(|| false);
    let mut fps_raw = use_signal(move || v_to_string(initial.max_fps));
    let mut fps_invalid = use_signal(|| false);
    let emit = initial.emit;
    let record_layer = initial.record_layer;

    let list = models.value().read().clone().flatten().unwrap_or_default();
    let loaded = models.value().read().is_some();
    let current = model_id();
    let model_labels: Vec<(String, bool)> = list
        .iter()
        .find(|m| m.id == current)
        .map(|m| {
            let picked = labels.read();
            m.spec
                .labels
                .iter()
                .map(|l| (l.clone(), picked.contains(l)))
                .collect()
        })
        .unwrap_or_default();
    let options: Vec<(String, String)> = list
        .iter()
        .map(|m| (m.id.clone(), m.name.clone()))
        .collect();
    let unknown_current = !current.is_empty() && !options.iter().any(|(id, _)| *id == current);
    let picked_count = labels.read().len();

    rsx! {
        div { class: "space-y-3",
            p { class: "text-xs text-gray-500", {t!("flows-vision-help")} }
            label { class: "block",
                span { class: FIELD_LABEL, {t!("flows-vision-model")} }
                select {
                    class: "w-full px-2 py-1 border border-gray-300 rounded-lg text-sm",
                    disabled: !can_write,
                    onchange: move |event| {
                        let id = event.value();
                        model_id.set(id.clone());
                        // Labels belong to a model: a new pick resets the filter.
                        labels.set(Vec::new());
                        patch_selected(
                            &mut cx,
                            move |node: &mut FlowNode| {
                                if let FlowNodeKind::VisionDetect { config } = &mut node.kind {
                                    config.model_id = id;
                                    config.labels.clear();
                                }
                            },
                        );
                    },
                    option { value: "", selected: current.is_empty(), {t!("flows-vision-model-none")} }
                    for (id, name) in options {
                        option {
                            key: "{id}",
                            value: "{id}",
                            selected: id == current,
                            "{name}"
                        }
                    }
                    if unknown_current {
                        option { value: "{current}", selected: true, "{current}" }
                    }
                }
            }
            if loaded && list.is_empty() {
                p { class: "text-xs text-amber-700", {t!("flows-vision-no-model")} }
            }
            if !model_labels.is_empty() {
                div {
                    span { class: "text-xs font-medium text-gray-500 mb-1 block",
                        {t!("flows-vision-labels", count : picked_count)}
                    }
                    div { class: "flex flex-wrap gap-1 max-h-40 overflow-y-auto",
                        for (label, on) in model_labels {
                            button {
                                key: "{label}",
                                r#type: "button",
                                disabled: !can_write,
                                class: if on { "px-2 py-0.5 rounded-full text-xs bg-fuchsia-600 text-white" } else { "px-2 py-0.5 rounded-full text-xs bg-gray-100 text-gray-700 hover:bg-gray-200" },
                                onclick: {
                                    let label = label.clone();
                                    move |_| {
                                        let label = label.clone();
                                        labels.with_mut(|l| toggle_label(l, &label));
                                        let next = labels.peek().clone();
                                        patch_selected(
                                            &mut cx,
                                            move |node: &mut FlowNode| {
                                                if let FlowNodeKind::VisionDetect { config } = &mut node.kind {
                                                    config.labels = next;
                                                }
                                            },
                                        );
                                    }
                                },
                                "{label}"
                            }
                        }
                    }
                    span { class: "text-xs text-gray-500 mt-1 block", {t!("flows-vision-labels-hint")} }
                }
            }
            label { class: "block",
                span { class: FIELD_LABEL, {t!("flows-vision-min-score")} }
                input {
                    class: if score_invalid() { "w-full px-2 py-1.5 border border-red-400 bg-red-50 rounded-lg text-sm" } else { "w-full px-2 py-1.5 border border-gray-300 rounded-lg text-sm" },
                    r#type: "number",
                    min: "0",
                    max: "1",
                    step: "0.05",
                    value: "{score_raw}",
                    disabled: !can_write,
                    oninput: move |event| {
                        let raw = event.value();
                        score_raw.set(raw.clone());
                        let parsed = if raw.trim().is_empty() { Some(0.0) } else { parse_secs(&raw) };
                        let valid = parsed.filter(|v| (0.0..=1.0).contains(v));
                        score_invalid.set(valid.is_none());
                        if let Some(v) = valid {
                            patch_selected(
                                &mut cx,
                                move |node: &mut FlowNode| {
                                    if let FlowNodeKind::VisionDetect { config } = &mut node.kind {
                                        config.min_score = v as f32;
                                    }
                                },
                            );
                        }
                    },
                }
                span { class: "text-xs text-gray-500 mt-1 block", {t!("flows-vision-min-score-hint")} }
            }
            label { class: "block",
                span { class: FIELD_LABEL, {t!("flows-vision-emit")} }
                select {
                    class: "w-full px-2 py-1 border border-gray-300 rounded-lg text-sm",
                    disabled: !can_write,
                    onchange: move |event| {
                        let next = if event.value() == "always" {
                            VisionEmit::Always
                        } else {
                            VisionEmit::OnDetection
                        };
                        patch_selected(
                            &mut cx,
                            move |node: &mut FlowNode| {
                                if let FlowNodeKind::VisionDetect { config } = &mut node.kind {
                                    config.emit = next;
                                }
                            },
                        );
                    },
                    option {
                        value: "on_detection",
                        selected: emit == VisionEmit::OnDetection,
                        {t!("flows-vision-emit-on-detection")}
                    }
                    option { value: "always", selected: emit == VisionEmit::Always,
                        {t!("flows-vision-emit-always")}
                    }
                }
            }
            label { class: "block",
                span { class: FIELD_LABEL, {t!("flows-camera-max-fps")} }
                input {
                    class: if fps_invalid() { "w-full px-2 py-1.5 border border-red-400 bg-red-50 rounded-lg text-sm" } else { "w-full px-2 py-1.5 border border-gray-300 rounded-lg text-sm" },
                    r#type: "number",
                    min: "0",
                    max: "25",
                    step: "0.5",
                    value: "{fps_raw}",
                    disabled: !can_write,
                    oninput: move |event| {
                        let raw = event.value();
                        fps_raw.set(raw.clone());
                        let parsed = if raw.trim().is_empty() { Some(0.0) } else { parse_secs(&raw) };
                        let valid = parsed
                            .filter(|v| (0.0..=pnex_core::CAMERA_NODE_MAX_FPS).contains(v));
                        fps_invalid.set(valid.is_none());
                        if let Some(v) = valid {
                            patch_selected(
                                &mut cx,
                                move |node: &mut FlowNode| {
                                    if let FlowNodeKind::VisionDetect { config } = &mut node.kind {
                                        config.max_fps = v;
                                    }
                                },
                            );
                        }
                    },
                }
                span { class: "text-xs text-gray-500 mt-1 block", {t!("flows-vision-max-fps-hint")} }
            }
            label { class: "flex items-start gap-2 text-sm text-gray-700 select-none",
                input {
                    class: "h-4 w-4 mt-0.5 accent-signal-600",
                    r#type: "checkbox",
                    checked: record_layer,
                    disabled: !can_write,
                    onchange: move |event| {
                        let next = event.checked();
                        patch_selected(
                            &mut cx,
                            move |node: &mut FlowNode| {
                                if let FlowNodeKind::VisionDetect { config } = &mut node.kind {
                                    config.record_layer = next;
                                }
                            },
                        );
                    },
                }
                span {
                    span { class: "block", {t!("flows-vision-record-layer")} }
                    span { class: "text-xs text-gray-500 block",
                        {t!("flows-vision-record-layer-hint")}
                    }
                }
            }
            ReviewSection { cx, initial: review_initial, can_write }
        }
    }
}

/// Which field of the doubt settings an input edits.
#[derive(Clone, Copy, PartialEq)]
enum ReviewField {
    Min,
    Max,
    Every,
}

impl ReviewField {
    fn apply(self, c: &mut VisionDetectConfig, v: f64) {
        match self {
            Self::Min => c.review_min = v as f32,
            Self::Max => c.review_max = v as f32,
            Self::Every => c.review_every_secs = v,
        }
    }
}

/// Active learning (M4): the Vision Lab project that receives the frames
/// where the model hesitated, the doubt band and the rate.
#[component]
fn ReviewSection(mut cx: EditorCx, initial: VisionDetectConfig, can_write: bool) -> Element {
    let projects = use_resource(|| async { api::lab::projects().await.unwrap_or_default() });
    let mut project = use_signal({
        let p = initial.review_project.clone();
        move || p
    });
    let list = projects.read().clone().unwrap_or_default();
    let on = !project().is_empty();
    rsx! {
        div { class: "space-y-2 border-t border-gray-100 pt-3",
            span { class: FIELD_LABEL, {t!("flows-vision-review")} }
            p { class: "text-xs text-gray-500", {t!("flows-vision-review-hint")} }
            select {
                class: "w-full px-2 py-1 border border-gray-300 rounded-lg text-sm",
                disabled: !can_write,
                onchange: move |event| {
                    let id = event.value();
                    project.set(id.clone());
                    patch_selected(
                        &mut cx,
                        move |node: &mut FlowNode| {
                            if let FlowNodeKind::VisionDetect { config } = &mut node.kind {
                                config.review_project = id;
                            }
                        },
                    );
                },
                option { value: "", selected: !on, {t!("flows-vision-review-off")} }
                for p in list {
                    option {
                        key: "{p.id}",
                        value: "{p.id}",
                        selected: p.id == project(),
                        "{p.name}"
                    }
                }
            }
            if on {
                div { class: "grid grid-cols-3 gap-2",
                    ReviewInput {
                        cx,
                        base: initial.clone(),
                        field: ReviewField::Min,
                        initial: f64::from(initial.review_min),
                        can_write,
                    }
                    ReviewInput {
                        cx,
                        base: initial.clone(),
                        field: ReviewField::Max,
                        initial: f64::from(initial.review_max),
                        can_write,
                    }
                    ReviewInput {
                        cx,
                        base: initial.clone(),
                        field: ReviewField::Every,
                        initial: initial.review_every_secs,
                        can_write,
                    }
                }
            }
        }
    }
}

#[component]
fn ReviewInput(
    mut cx: EditorCx,
    base: VisionDetectConfig,
    field: ReviewField,
    initial: f64,
    can_write: bool,
) -> Element {
    let mut raw = use_signal(move || v_to_string(initial));
    let mut invalid = use_signal(|| false);
    let label = match field {
        ReviewField::Min => t!("flows-vision-review-min"),
        ReviewField::Max => t!("flows-vision-review-max"),
        ReviewField::Every => t!("flows-vision-review-every"),
    };
    let class = if invalid() {
        "w-full px-2 py-1.5 border border-red-400 bg-red-50 rounded-lg text-sm"
    } else {
        "w-full px-2 py-1.5 border border-gray-300 rounded-lg text-sm"
    };
    rsx! {
        label { class: "block",
            span { class: "text-xs text-gray-600", {label} }
            input {
                class,
                r#type: "number",
                step: if field == ReviewField::Every { "5" } else { "0.05" },
                value: "{raw}",
                disabled: !can_write,
                oninput: move |event| {
                    let next = event.value();
                    raw.set(next.clone());
                    // Checked on the base config with only this field changed.
                    let mut probe = base.clone();
                    if probe.review_project.is_empty() {
                        probe.review_project = "00000000-0000-0000-0000-000000000000".into();
                    }
                    let ok = parse_secs(&next)
                        .filter(|v| {
                            field.apply(&mut probe, *v);
                            probe.check().is_none_or(|(code, _)| !code.starts_with("vision_review"))
                        });
                    invalid.set(ok.is_none());
                    if let Some(v) = ok {
                        patch_selected(
                            &mut cx,
                            move |node: &mut FlowNode| {
                                if let FlowNodeKind::VisionDetect { config } = &mut node.kind {
                                    field.apply(config, v);
                                }
                            },
                        );
                    }
                },
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn toggle_label_adds_then_removes() {
        let mut labels = vec!["person".to_string()];
        toggle_label(&mut labels, "dog");
        assert_eq!(labels, vec!["person", "dog"]);
        toggle_label(&mut labels, "person");
        assert_eq!(labels, vec!["dog"]);
    }
}
