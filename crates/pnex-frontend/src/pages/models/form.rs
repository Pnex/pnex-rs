//! Create / edit modal of a vision model: name, ONNX source (existing
//! `model` media asset or direct upload — create only) and inference spec.
//!
//! D100: the input size an ONNX file imposes is read from the file
//! (`GET /ml/models/inspect`) and locked; the server imposes it anyway on
//! save (upload case) and refuses a spec the model cannot run.

use dioxus::prelude::*;
use dioxus_i18n::t;
use pnex_core::vision::{coco_labels, MlModel, ModelSpec, VisionFamily};

use crate::api;
use crate::api::error::ApiError;
use crate::api::media::{MediaFilters, MediaKind, UploadParams};
use crate::components::crud::layout::{GHOST_BTN, PRIMARY_BTN};
use crate::components::modal::Modal;
use crate::state::toasts;

/// Where the ONNX bytes come from (create only).
#[derive(Clone, Copy, PartialEq, Eq)]
enum Source {
    Existing,
    Upload,
}

/// One label per line → label list (blank lines dropped, trimmed).
pub(super) fn parse_labels(raw: &str) -> Vec<String> {
    raw.lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .map(str::to_string)
        .collect()
}

/// Upload file name: keeps the picked name, forces the `.onnx` extension
/// the server sniffs to classify the media as kind `model`.
pub(super) fn onnx_filename(name: &str) -> String {
    let name = name.trim();
    if name.to_ascii_lowercase().ends_with(".onnx") {
        name.to_string()
    } else if name.is_empty() {
        "model.onnx".to_string()
    } else {
        format!("{name}.onnx")
    }
}

const INPUT_CLASS: &str = "w-full px-3 py-2 border border-gray-300 rounded-lg text-sm";
const LABEL_CLASS: &str = "block text-sm font-medium text-gray-700 mb-1";

#[component]
pub(super) fn ModelFormModal(
    existing: Option<MlModel>,
    on_close: Callback<()>,
    on_saved: Callback<()>,
) -> Element {
    let is_edit = existing.is_some();
    let edit_id = existing.as_ref().map(|m| m.id.clone());
    let spec0 = existing
        .as_ref()
        .map(|m| m.spec.clone())
        .unwrap_or_default();
    let name0 = existing
        .as_ref()
        .map(|m| m.name.clone())
        .unwrap_or_default();
    let desc0 = existing
        .as_ref()
        .map(|m| m.description.clone())
        .unwrap_or_default();

    let mut name = use_signal(move || name0.clone());
    let mut description = use_signal(move || desc0.clone());
    let mut family = use_signal(move || spec0.family);
    let mut width = use_signal(move || spec0.input_width.to_string());
    let mut height = use_signal(move || spec0.input_height.to_string());
    let score = use_signal(move || spec0.score_threshold.to_string());
    let nms = use_signal(move || spec0.nms_iou.to_string());
    let mut labels = use_signal(move || spec0.labels.join("\n"));
    let mut source = use_signal(|| Source::Upload);
    let mut asset_id = use_signal(String::new);
    let edit_asset = existing.as_ref().map(|m| m.asset_id.clone());
    let mut file_name = use_signal(String::new);
    let mut file_bytes = use_signal(|| None::<Vec<u8>>);
    let mut field_errors = use_signal(Vec::<(String, String)>::new);
    let mut save_error = use_signal(|| None::<ApiError>);
    let mut saving = use_signal(|| false);

    let assets = use_resource(move || async move {
        let filters = MediaFilters {
            kinds: vec![MediaKind::Model],
            limit: Some(100),
            ..Default::default()
        };
        api::media::list(&filters)
            .await
            .map(|p| p.results)
            .unwrap_or_default()
    });
    // What the picked (or edited) ONNX file declares — locks the input size.
    let inspection = use_resource(move || {
        let target = edit_asset.clone().unwrap_or_else(|| asset_id());
        async move {
            if target.is_empty() {
                return None;
            }
            api::ml_models::inspect(&target).await.ok()
        }
    });
    let fixed_input = inspection
        .value()
        .read()
        .clone()
        .flatten()
        .and_then(|i| i.fixed_input());
    let file_classes = inspection
        .value()
        .read()
        .clone()
        .flatten()
        .and_then(|i| i.classes);
    use_effect(move || {
        if let Some((w, h)) = inspection
            .value()
            .read()
            .clone()
            .flatten()
            .and_then(|i| i.fixed_input())
        {
            width.set(w.to_string());
            height.set(h.to_string());
        }
    });
    let asset_options: Vec<(String, String)> = assets
        .value()
        .read()
        .clone()
        .unwrap_or_default()
        .into_iter()
        .map(|a| (a.id, a.name))
        .collect();

    let error_of = move |field: &str| -> Option<String> {
        field_errors
            .read()
            .iter()
            .find(|(f, _)| f == field)
            .map(|(f, token)| crate::api::error_i18n::localize_field(f, token))
    };
    let errors: Vec<(&'static str, Option<String>)> = [
        "name",
        "asset_id",
        "input_width",
        "input_height",
        "score_threshold",
        "nms_iou",
        "labels",
    ]
    .into_iter()
    .map(|f| (f, error_of(f)))
    .collect();
    let err = move |f: &str| {
        errors
            .iter()
            .find(|(k, _)| *k == f)
            .and_then(|(_, e)| e.clone())
    };
    let name_err = err("name");
    let asset_err = err("asset_id");
    let width_err = err("input_width");
    let height_err = err("input_height");
    let score_err = err("score_threshold");
    let nms_err = err("nms_iou");
    let labels_err = err("labels");
    let label_count = parse_labels(&labels()).len();

    let submit = move |_| {
        let saved_msg = t!("models-saved").to_string();
        // Local number parsing; the ranges stay with ModelSpec::check (server).
        let mut local = Vec::new();
        let w = width.peek().trim().parse::<u32>().ok();
        let h = height.peek().trim().parse::<u32>().ok();
        let s = score.peek().trim().parse::<f32>().ok();
        let n = nms.peek().trim().parse::<f32>().ok();
        for (field, ok, token) in [
            ("input_width", w.is_some(), "range:32..2048/32"),
            ("input_height", h.is_some(), "range:32..2048/32"),
            ("score_threshold", s.is_some(), "range:0..1"),
            ("nms_iou", n.is_some(), "range:0..1"),
        ] {
            if !ok {
                local.push((field.to_string(), token.to_string()));
            }
        }
        if !local.is_empty() {
            field_errors.set(local);
            return;
        }
        let spec = ModelSpec {
            family: *family.peek(),
            input_width: w.unwrap_or_default(),
            input_height: h.unwrap_or_default(),
            labels: parse_labels(&labels.peek()),
            score_threshold: s.unwrap_or_default(),
            nms_iou: n.unwrap_or_default(),
        };
        if let Err(errs) = spec.check() {
            field_errors.set(errs.into_iter().map(|(f, t)| (f.to_string(), t)).collect());
            return;
        }
        let name_v = name.peek().trim().to_string();
        let desc_v = description.peek().trim().to_string();
        let edit_id = edit_id.clone();
        let src = *source.peek();
        let picked_asset = asset_id.peek().clone();
        let upload_name = onnx_filename(&file_name.peek());
        let bytes = file_bytes.peek().clone();
        spawn(async move {
            saving.set(true);
            field_errors.set(Vec::new());
            save_error.set(None);
            let result = match edit_id {
                Some(id) => {
                    let input = api::ml_models::UpdateModel {
                        name: Some(name_v),
                        description: Some(desc_v),
                        spec: Some(spec),
                    };
                    api::ml_models::update(&id, &input).await.map(|_| ())
                }
                None => {
                    // Resolve the ONNX asset: picked, or uploaded now.
                    let asset = match src {
                        Source::Existing => Ok(picked_asset),
                        Source::Upload => match bytes {
                            Some(bytes) => {
                                let params = UploadParams {
                                    name: Some(if name_v.is_empty() {
                                        upload_name.clone()
                                    } else {
                                        name_v.clone()
                                    }),
                                    filename: Some(upload_name),
                                    kind: Some(MediaKind::Model),
                                    content_type: None,
                                    note: None,
                                };
                                api::media::upload(&params, bytes).await.map(|a| a.id)
                            }
                            None => Ok(String::new()),
                        },
                    };
                    match asset {
                        Ok(asset_id) if asset_id.is_empty() => {
                            field_errors
                                .set(vec![("asset_id".to_string(), "required".to_string())]);
                            saving.set(false);
                            return;
                        }
                        Ok(asset_id) => {
                            let input = api::ml_models::CreateModel {
                                name: name_v,
                                description: Some(desc_v).filter(|d| !d.is_empty()),
                                asset_id,
                                spec,
                            };
                            api::ml_models::create(&input).await.map(|_| ())
                        }
                        Err(e) => Err(e),
                    }
                }
            };
            match result {
                Ok(()) => {
                    toasts::success(saved_msg);
                    on_saved.call(());
                }
                Err(e) => {
                    let fields = api::ml_models::field_errors(&e);
                    if fields.is_empty() {
                        save_error.set(Some(e));
                    } else {
                        field_errors.set(fields);
                    }
                }
            }
            saving.set(false);
        });
    };

    let title = if is_edit {
        t!("models-edit-title")
    } else {
        t!("models-create-title")
    };

    rsx! {
        Modal { title, max_width: "max-w-2xl".to_string(), on_close: move |_| on_close.call(()),
            div { class: "space-y-4",
                div { class: "grid gap-4 md:grid-cols-2",
                    div {
                        label { class: LABEL_CLASS, {t!("models-name")} }
                        input { class: INPUT_CLASS, value: "{name}", oninput: move |e| name.set(e.value()) }
                        if let Some(e) = name_err {
                            span { class: "text-xs text-red-600", {e} }
                        }
                    }
                    div {
                        label { class: LABEL_CLASS, {t!("models-description")} }
                        input { class: INPUT_CLASS, value: "{description}", oninput: move |e| description.set(e.value()) }
                    }
                }
                if !is_edit {
                    div { class: "space-y-2",
                        span { class: LABEL_CLASS, {t!("models-source")} }
                        div { class: "flex gap-4 text-sm",
                            label { class: "flex items-center gap-2",
                                input {
                                    r#type: "radio",
                                    checked: source() == Source::Upload,
                                    onchange: move |_| source.set(Source::Upload),
                                }
                                {t!("models-source-upload")}
                            }
                            label { class: "flex items-center gap-2",
                                input {
                                    r#type: "radio",
                                    checked: source() == Source::Existing,
                                    onchange: move |_| source.set(Source::Existing),
                                }
                                {t!("models-source-existing")}
                            }
                        }
                        if source() == Source::Upload {
                            input {
                                class: INPUT_CLASS,
                                r#type: "file",
                                accept: ".onnx",
                                onchange: move |evt| {
                                    if let Some(file) = evt.files().first().cloned() {
                                        file_name.set(file.name());
                                        spawn(async move {
                                            if let Ok(bytes) = file.read_bytes().await {
                                                file_bytes.set(Some(bytes.to_vec()));
                                            }
                                        });
                                    }
                                },
                            }
                        } else {
                            select {
                                class: "{INPUT_CLASS} bg-white",
                                onchange: move |e| asset_id.set(e.value()),
                                option { value: "", selected: asset_id().is_empty(), {t!("models-source-pick")} }
                                for (id , label) in asset_options {
                                    option { key: "{id}", value: "{id}", selected: id == asset_id(), "{label}" }
                                }
                            }
                        }
                        if let Some(e) = asset_err {
                            span { class: "text-xs text-red-600 block", {e} }
                        }
                        p { class: "text-xs text-gray-500", {t!("models-license-hint")} }
                    }
                }
                div { class: "grid gap-4 grid-cols-2 md:grid-cols-5",
                    div {
                        label { class: LABEL_CLASS, {t!("models-family")} }
                        select {
                            class: "{INPUT_CLASS} bg-white",
                            onchange: move |e| {
                                if let Some(f) = VisionFamily::from_wire(&e.value()) {
                                    family.set(f);
                                }
                            },
                            for f in VisionFamily::ALL {
                                option { key: "{f.wire()}", value: f.wire(), selected: family() == f, {f.wire().to_uppercase()} }
                            }
                        }
                    }
                    {number_field(t!("models-input-width"), width, width_err, fixed_input.is_some())}
                    {number_field(t!("models-input-height"), height, height_err, fixed_input.is_some())}
                    {number_field(t!("models-score-threshold"), score, score_err, false)}
                    {number_field(t!("models-nms-iou"), nms, nms_err, false)}
                }
                if let Some((w, h)) = fixed_input {
                    p { class: "text-xs text-emerald-700", {t!("models-input-from-file", width: w, height: h)} }
                } else if !is_edit && source() == Source::Upload {
                    p { class: "text-xs text-gray-500", {t!("models-input-auto-hint")} }
                }
                p { class: "text-xs text-gray-500", {t!("models-spec-hint")} }
                div {
                    div { class: "flex items-center justify-between mb-1",
                        label { class: "text-sm font-medium text-gray-700", {t!("models-labels", count: label_count)} }
                        button {
                            class: "text-xs text-blue-600 hover:underline",
                            r#type: "button",
                            onclick: move |_| labels.set(coco_labels().join("\n")),
                            {t!("models-labels-coco")}
                        }
                    }
                    textarea {
                        class: "{INPUT_CLASS} h-40 font-mono",
                        value: "{labels}",
                        oninput: move |e| labels.set(e.value()),
                    }
                    if let Some(e) = labels_err {
                        span { class: "text-xs text-red-600", {e} }
                    } else if let Some(classes) = file_classes.filter(|c| *c as usize != label_count) {
                        span { class: "text-xs text-amber-700", {t!("models-labels-mismatch", classes: classes, count: label_count)} }
                    }
                }
                if let Some(e) = save_error() {
                    div { class: "rounded-lg bg-red-50 border border-red-200 p-2 text-sm text-red-700",
                        {crate::api::error_i18n::localize(&e)}
                    }
                }
                div { class: "flex justify-end gap-2",
                    button { class: GHOST_BTN, r#type: "button", onclick: move |_| on_close.call(()), {t!("common-cancel")} }
                    button {
                        class: PRIMARY_BTN,
                        r#type: "button",
                        disabled: saving(),
                        onclick: submit,
                        if saving() {
                            {t!("models-saving")}
                        } else {
                            {t!("models-save")}
                        }
                    }
                }
            }
        }
    }
}

/// Labeled numeric text input with an optional localized field error;
/// `locked` = value dictated by the model file (read-only).
fn number_field(
    label: String,
    mut value: Signal<String>,
    error: Option<String>,
    locked: bool,
) -> Element {
    rsx! {
        div {
            label { class: LABEL_CLASS, {label} }
            input {
                class: INPUT_CLASS,
                r#type: "number",
                step: "any",
                disabled: locked,
                value: "{value}",
                oninput: move |e| value.set(e.value()),
            }
            if let Some(e) = error {
                span { class: "text-xs text-red-600", {e} }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn labels_one_per_line() {
        assert_eq!(parse_labels(" person \n\n dog\r\n"), vec!["person", "dog"]);
    }

    #[test]
    fn onnx_extension_forced() {
        assert_eq!(onnx_filename("yolox_nano.onnx"), "yolox_nano.onnx");
        assert_eq!(onnx_filename("yolox"), "yolox.onnx");
        assert_eq!(onnx_filename(""), "model.onnx");
    }
}
