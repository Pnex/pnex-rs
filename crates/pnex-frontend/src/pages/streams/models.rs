//! "Models and profiles" tab of /streams (media-ingest.md D164, D167):
//! speech-to-text models of the org (import, check, delete) and the
//! transcription profiles that streams point at.

use dioxus::prelude::*;
use dioxus_i18n::t;
use pnex_core::media_ingest::{
    license_is_non_commercial, AsrModel, AsrModelInput, AsrProfileInput, ASR_LICENSES,
};

use crate::api;
use crate::api::media::{MediaFilters, MediaKind, UploadParams};
use crate::components::confirm::ConfirmDialog;
use crate::components::crud::form::FormDialog;
use crate::components::crud::layout::DANGER_BTN;
use crate::state::toasts;

const INPUT: &str = "w-full px-3 py-2 border border-gray-300 rounded-lg text-sm bg-white";
const LABEL: &str = "block text-xs text-gray-600 mb-1";
const BTN: &str =
    "px-2 py-1 text-xs border border-gray-300 rounded-lg hover:bg-gray-50 disabled:opacity-40";

/// What waits for a confirmation.
#[derive(Clone, PartialEq)]
enum Pending {
    Model(AsrModel),
    Profile(String, String),
}

#[component]
pub fn ModelsTab(can_write: bool, reload: Signal<u32>, importing: Signal<bool>) -> Element {
    let mut reload = reload;
    let mut importing = importing;
    let mut pending = use_signal(|| None::<Pending>);
    let mut checking = use_signal(|| None::<String>);
    let mut testing = use_signal(|| None::<AsrModel>);
    let models = use_resource(move || async move {
        let _ = reload();
        api::media_streams::models().await
    });
    let profiles = use_resource(move || async move {
        let _ = reload();
        api::media_streams::profiles().await
    });
    let model_rows: Vec<AsrModel> = match &*models.value().read() {
        Some(Ok(rows)) => rows.clone(),
        _ => Vec::new(),
    };
    let profile_rows = match &*profiles.value().read() {
        Some(Ok(rows)) => rows.clone(),
        _ => Vec::new(),
    };
    let model_name = {
        let rows = model_rows.clone();
        move |id: &str| {
            rows.iter()
                .find(|m| m.id == id)
                .map(|m| m.name.clone())
                .unwrap_or_default()
        }
    };

    rsx! {
        div { class: "space-y-6",
            section { class: "space-y-2",
                h3 { class: "text-sm font-semibold text-gray-900", {t!("asr-models-title")} }
                p { class: "text-xs text-gray-500", {t!("asr-models-help")} }
                if model_rows.is_empty() {
                    p { class: "text-sm text-gray-500", {t!("asr-models-empty")} }
                } else {
                    div { class: "overflow-x-auto bg-white rounded-lg shadow border border-gray-200",
                        table { class: "min-w-full divide-y divide-gray-200 text-sm",
                            thead { class: "bg-gray-50",
                                tr {
                                    th { class: "th", {t!("asr-models-col-name")} }
                                    th { class: "th hidden md:table-cell",
                                        {t!("asr-models-col-family")}
                                    }
                                    th { class: "th", {t!("asr-models-col-check")} }
                                    th { class: "th" }
                                }
                            }
                            tbody { class: "divide-y divide-gray-100",
                                for m in model_rows.clone() {
                                    ModelRow {
                                        key: "{m.id}",
                                        model: m.clone(),
                                        can_write,
                                        checking: checking() == Some(m.id.clone()),
                                        on_check: move |id: String| {
                                            checking.set(Some(id.clone()));
                                            spawn(async move {
                                                match api::media_streams::check_model(&id).await {
                                                    Ok(_) => reload.with_mut(|r| *r += 1),
                                                    Err(err) => toasts::error(err),
                                                }
                                                checking.set(None);
                                            });
                                        },
                                        on_delete: move |m: AsrModel| pending.set(Some(Pending::Model(m))),
                                        on_test: move |m: AsrModel| testing.set(Some(m)),
                                    }
                                }
                            }
                        }
                    }
                }
            }
            section { class: "space-y-2",
                h3 { class: "text-sm font-semibold text-gray-900", {t!("asr-profiles-title")} }
                p { class: "text-xs text-gray-500", {t!("asr-profiles-help")} }
                if profile_rows.is_empty() {
                    p { class: "text-sm text-gray-500", {t!("asr-profiles-empty")} }
                } else {
                    ul { class: "divide-y divide-gray-100 bg-white rounded-lg shadow border border-gray-200",
                        for p in profile_rows {
                            li {
                                key: "{p.id}",
                                class: "flex items-center justify-between gap-2 px-4 py-2 text-sm",
                                div {
                                    p { class: "font-medium text-gray-900", "{p.name}" }
                                    p { class: "text-xs text-gray-500",
                                        {format!("{} · {}", model_name(&p.asr_model_id), p.language)}
                                    }
                                }
                                if can_write {
                                    button {
                                        class: DANGER_BTN,
                                        r#type: "button",
                                        onclick: {
                                            let (id, name) = (p.id.clone(), p.name.clone());
                                            move |_| pending.set(Some(Pending::Profile(id.clone(), name.clone())))
                                        },
                                        {t!("streams-delete")}
                                    }
                                }
                            }
                        }
                    }
                }
                if can_write && !model_rows.is_empty() {
                    NewProfile {
                        models: model_rows.clone(),
                        on_saved: move |_| reload.with_mut(|r| *r += 1),
                    }
                }
            }
        }
        if importing() {
            ImportModel {
                on_close: move |_| importing.set(false),
                on_saved: move |_| {
                    importing.set(false);
                    reload.with_mut(|r| *r += 1);
                },
            }
        }
        if let Some(m) = testing() {
            TestModel {
                key: "test-{m.id}",
                model: m,
                on_close: move |_| testing.set(None),
            }
        }
        if let Some(p) = pending() {
            ConfirmDialog {
                title: t!("asr-delete-title"),
                message: match &p {
                    Pending::Model(m) => t!("asr-models-delete-message", name : m.name.clone()),
                    Pending::Profile(_, name) => {
                        t!("asr-profiles-delete-message", name : name.clone())
                    }
                },
                confirm_label: t!("streams-delete-confirm"),
                on_confirm: move |_| {
                    let p = p.clone();
                    pending.set(None);
                    spawn(async move {
                        let res = match &p {
                            Pending::Model(m) => api::media_streams::delete_model(&m.id).await,
                            Pending::Profile(id, _) => api::media_streams::delete_profile(id).await,
                        };
                        match res {
                            Ok(_) => reload.with_mut(|r| *r += 1),
                            Err(err) => toasts::error(err),
                        }
                    });
                },
                on_cancel: move |_| pending.set(None),
            }
        }
    }
}

#[component]
fn ModelRow(
    model: AsrModel,
    can_write: bool,
    checking: bool,
    on_check: Callback<String>,
    on_delete: Callback<AsrModel>,
    on_test: Callback<AsrModel>,
) -> Element {
    let check = model.check.clone();
    let (badge_class, badge) = match check.status.as_str() {
        "valid" => ("bg-emerald-100 text-emerald-800", t!("models-check-valid")),
        "invalid" => ("bg-red-100 text-red-800", t!("models-check-invalid")),
        _ => ("bg-gray-100 text-gray-700", t!("models-check-unchecked")),
    };
    let measure = check.streams().zip(check.wer).map(|(streams, wer)| {
        t!(
            "asr-models-measure",
            streams: format!("{streams:.0}"),
            wer: format!("{:.1}", wer * 100.0)
        )
    });
    let carrier_lines: Vec<String> = model
        .carriers
        .iter()
        .map(|c| match (c.check.status.as_str(), c.check.streams()) {
            ("valid", Some(streams)) => t!(
                "asr-models-carrier-valid",
                carrier: c.carrier.clone(),
                streams: format!("{streams:.0}")
            )
            .to_string(),
            _ => t!("asr-models-carrier-invalid", carrier: c.carrier.clone()).to_string(),
        })
        .collect();
    let nc = license_is_non_commercial(&model.license);
    let id = model.id.clone();
    let m_delete = model.clone();
    let m_test = model.clone();
    let valid = check.status == "valid";
    rsx! {
        tr {
            td { class: "td",
                p { class: "font-medium text-gray-900", "{model.name}" }
                p { class: if nc { "text-xs text-amber-700" } else { "text-xs text-gray-500" },
                    if nc {
                        {t!("asr-models-license-nc", license : model.license.clone())}
                    } else {
                        "{model.license}"
                    }
                }
            }
            td { class: "td hidden text-gray-600 md:table-cell", "{model.family}" }
            td { class: "td",
                span { class: "inline-flex px-2 py-0.5 rounded text-xs font-medium {badge_class}",
                    {badge}
                }
                if let Some(m) = measure {
                    p { class: "text-xs text-gray-500 mt-0.5", {m} }
                }
                if let Some(err) = check.error.clone() {
                    p { class: "text-xs text-red-700 mt-0.5 break-words", "{err}" }
                }
                for line in carrier_lines.iter() {
                    p { class: "text-xs text-gray-500 mt-0.5", "{line}" }
                }
            }
            td { class: "td text-right",
                if can_write {
                    div { class: "flex justify-end gap-2",
                        if valid {
                            button {
                                class: BTN,
                                r#type: "button",
                                onclick: move |_| on_test.call(m_test.clone()),
                                {t!("asr-models-test")}
                            }
                        }
                        button {
                            class: BTN,
                            r#type: "button",
                            disabled: checking,
                            onclick: move |_| on_check.call(id.clone()),
                            if checking {
                                {t!("asr-models-checking")}
                            } else {
                                {t!("asr-models-check")}
                            }
                        }
                        button {
                            class: DANGER_BTN,
                            r#type: "button",
                            onclick: move |_| on_delete.call(m_delete.clone()),
                            {t!("streams-delete")}
                        }
                    }
                }
            }
        }
    }
}

/// Inline profile creation: name, model, language.
#[component]
fn NewProfile(models: Vec<AsrModel>, on_saved: Callback<()>) -> Element {
    let first = models.first().map(|m| m.id.clone()).unwrap_or_default();
    let mut name = use_signal(String::new);
    let mut model = use_signal(move || first.clone());
    let mut language = use_signal(|| "fr".to_string());
    let mut busy = use_signal(|| false);
    let save = move |_| {
        let input = AsrProfileInput {
            name: Some(name()),
            asr_model_id: Some(model()),
            language: Some(language()),
            ..Default::default()
        };
        busy.set(true);
        spawn(async move {
            let res = api::media_streams::create_profile(&input).await;
            busy.set(false);
            match res {
                Ok(_) => {
                    name.set(String::new());
                    toasts::success(t!("asr-profiles-saved").to_string());
                    on_saved.call(());
                }
                Err(err) => toasts::error(err),
            }
        });
    };
    rsx! {
        div { class: "grid grid-cols-1 sm:grid-cols-4 gap-2 items-end",
            div {
                label { r#for: "profile-name", class: LABEL, {t!("asr-profiles-name")} }
                input {
                    id: "profile-name",
                    class: INPUT,
                    value: "{name}",
                    oninput: move |e| name.set(e.value()),
                }
            }
            div {
                label { r#for: "profile-model", class: LABEL, {t!("asr-profiles-model")} }
                select {
                    id: "profile-model",
                    class: INPUT,
                    onchange: move |e| model.set(e.value()),
                    for m in models {
                        option { value: "{m.id}", selected: model() == m.id, "{m.name}" }
                    }
                }
            }
            div {
                label { r#for: "profile-language", class: LABEL, {t!("asr-profiles-language")} }
                input {
                    id: "profile-language",
                    class: INPUT,
                    value: "{language}",
                    maxlength: "4",
                    oninput: move |e| language.set(e.value()),
                }
            }
            button {
                class: "px-4 py-2 bg-blue-600 text-white rounded-lg hover:bg-blue-700 text-sm disabled:opacity-40",
                r#type: "button",
                disabled: busy() || name().trim().is_empty(),
                onclick: save,
                {t!("asr-profiles-add")}
            }
        }
    }
}

/// Where the model file comes from.
#[derive(Clone, Copy, PartialEq)]
enum Source {
    Upload,
    Library,
}

#[component]
fn ImportModel(on_close: Callback<()>, on_saved: Callback<()>) -> Element {
    let mut name = use_signal(String::new);
    let mut license = use_signal(|| "Apache-2.0".to_string());
    let mut source = use_signal(|| Source::Upload);
    let mut file_name = use_signal(String::new);
    let mut file_bytes = use_signal(|| None::<Vec<u8>>);
    let mut asset_id = use_signal(String::new);
    let mut busy = use_signal(|| false);
    let library = use_resource(|| async move {
        let filters = MediaFilters {
            kinds: vec![MediaKind::Model],
            limit: Some(100),
            ..Default::default()
        };
        api::media::list(&filters).await
    });
    let assets = match &*library.value().read() {
        Some(Ok(p)) => p.results.clone(),
        _ => Vec::new(),
    };
    let save = move |_| {
        let src = source();
        let bytes = file_bytes.peek().clone();
        let fname = file_name();
        let picked = asset_id();
        let name_v = name();
        let license_v = license();
        busy.set(true);
        spawn(async move {
            let asset = match (src, bytes) {
                (Source::Upload, Some(bytes)) => {
                    let params = UploadParams {
                        name: Some(if name_v.trim().is_empty() {
                            fname.clone()
                        } else {
                            name_v.clone()
                        }),
                        filename: Some(fname),
                        kind: Some(MediaKind::Model),
                        content_type: None,
                        note: None,
                    };
                    api::media::upload(&params, bytes).await.map(|a| a.id)
                }
                (Source::Upload, None) => Ok(String::new()),
                (Source::Library, _) => Ok(picked),
            };
            let res = match asset {
                Ok(id) => {
                    let input = AsrModelInput {
                        name: Some(name_v),
                        asset_id: Some(id),
                        license: Some(license_v),
                        ..Default::default()
                    };
                    api::media_streams::create_model(&input).await.map(|_| ())
                }
                Err(e) => Err(e),
            };
            busy.set(false);
            match res {
                Ok(()) => {
                    toasts::success(t!("asr-models-imported").to_string());
                    on_saved.call(());
                }
                Err(err) => toasts::error(err),
            }
        });
    };
    let ready = !name().trim().is_empty()
        && match source() {
            Source::Upload => file_bytes.read().is_some(),
            Source::Library => !asset_id().is_empty(),
        };
    rsx! {
        FormDialog {
            title: t!("asr-models-import-title").to_string(),
            submit_label: if busy() { t!("asr-models-importing").to_string() } else { t!("asr-models-import").to_string() },
            on_close,
            on_submit: save,
            busy: busy(),
            valid: ready,
            max_width: "max-w-xl".to_string(),
            p { class: "text-xs text-gray-500", {t!("asr-models-import-help")} }
            div {
                label { r#for: "asr-name", class: LABEL, {t!("asr-models-col-name")} }
                input {
                    id: "asr-name",
                    class: INPUT,
                    value: "{name}",
                    oninput: move |e| name.set(e.value()),
                }
            }
            div { class: "flex gap-4 text-sm",
                label { class: "inline-flex items-center gap-2",
                    input {
                        r#type: "radio",
                        name: "asr-source",
                        checked: source() == Source::Upload,
                        onchange: move |_| source.set(Source::Upload),
                    }
                    {t!("asr-models-source-upload")}
                }
                label { class: "inline-flex items-center gap-2",
                    input {
                        r#type: "radio",
                        name: "asr-source",
                        checked: source() == Source::Library,
                        onchange: move |_| source.set(Source::Library),
                    }
                    {t!("asr-models-source-library")}
                }
            }
            if source() == Source::Upload {
                input {
                    r#type: "file",
                    accept: ".tar,.bz2,.gz,.tgz,.zip,.bin,.gguf",
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
                select { class: INPUT, onchange: move |e| asset_id.set(e.value()),
                    option { value: "", selected: asset_id().is_empty(), {t!("models-source-pick")} }
                    for a in assets {
                        option { value: "{a.id}", selected: asset_id() == a.id, "{a.name}" }
                    }
                }
            }
            div {
                label { r#for: "asr-license", class: LABEL, {t!("asr-models-license")} }
                select {
                    id: "asr-license",
                    class: INPUT,
                    onchange: move |e| license.set(e.value()),
                    for l in ASR_LICENSES {
                        option { value: l, selected: license() == l, {l} }
                    }
                }
            }
        }
    }
}

/// Test of a checked model on a dropped clip (D167): transcription,
/// timings and how long it took.
#[component]
fn TestModel(model: AsrModel, on_close: Callback<()>) -> Element {
    let mut file_name = use_signal(String::new);
    let mut file_bytes = use_signal(|| None::<Vec<u8>>);
    let mut language = use_signal(|| "fr".to_string());
    let mut busy = use_signal(|| false);
    let mut result = use_signal(|| None::<pnex_core::media_ingest::AsrTestResult>);
    let id = model.id.clone();
    let run = move |_| {
        let Some(bytes) = file_bytes.peek().clone() else {
            return;
        };
        let id = id.clone();
        let name = file_name();
        let lang = language();
        busy.set(true);
        result.set(None);
        spawn(async move {
            let res = api::media_streams::test_model(&id, &name, &lang, bytes).await;
            busy.set(false);
            match res {
                Ok(r) => result.set(Some(r)),
                Err(err) => toasts::error(err),
            }
        });
    };
    let summary = result().map(|r| {
        let audio = format!("{:.1}", r.audio_ms as f64 / 1000.0);
        let infer = format!("{:.1}", r.infer_ms as f64 / 1000.0);
        t!("asr-test-summary", audio: audio, infer: infer).to_string()
    });
    rsx! {
        FormDialog {
            title: t!("asr-test-title", name : model.name.clone()).to_string(),
            submit_label: t!("asr-test-run").to_string(),
            on_close,
            on_submit: run,
            busy: busy(),
            valid: file_bytes.read().is_some(),
            max_width: "max-w-2xl".to_string(),
            p { class: "text-xs text-gray-500", {t!("asr-test-help")} }
            div { class: "grid grid-cols-1 sm:grid-cols-3 gap-3",
                div { class: "sm:col-span-2",
                    label { r#for: "asr-test-file", class: LABEL, {t!("asr-test-file")} }
                    input {
                        id: "asr-test-file",
                        class: INPUT,
                        r#type: "file",
                        accept: "audio/*,video/mp4,.mp3,.aac,.ogg,.opus,.flac,.wav,.m4a,.mp4,.ts",
                        onchange: move |evt| async move {
                            if let Some(file) = evt.files().first().cloned() {
                                file_name.set(file.name());
                                if let Ok(bytes) = file.read_bytes().await {
                                    file_bytes.set(Some(bytes.to_vec()));
                                }
                            }
                        },
                    }
                }
                div {
                    label { r#for: "asr-test-language", class: LABEL, {t!("asr-profiles-language")} }
                    input {
                        id: "asr-test-language",
                        class: INPUT,
                        r#type: "text",
                        maxlength: "4",
                        value: "{language}",
                        oninput: move |e| language.set(e.value()),
                    }
                }
            }
            if let Some(r) = result() {
                div { class: "rounded-lg border border-gray-200 bg-gray-50 p-3 space-y-2",
                    if let Some(sum) = summary {
                        p { class: "text-xs text-gray-600", "{sum}" }
                    }
                    if r.truncated {
                        p { class: "text-xs text-amber-700", {t!("asr-test-truncated")} }
                    }
                    if r.text.trim().is_empty() {
                        p { class: "text-sm text-gray-500", {t!("asr-test-no-speech")} }
                    } else {
                        p { class: "text-sm text-gray-900 whitespace-pre-wrap", "{r.text}" }
                    }
                }
            }
        }
    }
}
