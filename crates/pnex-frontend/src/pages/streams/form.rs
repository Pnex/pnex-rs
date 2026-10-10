//! Create / edit dialog of a media stream (media-ingest.md D159, D161,
//! D172). The URL never carries a credential: it goes in the vault secret
//! (typed or picked, owner/admin only).

use dioxus::prelude::*;
use dioxus_i18n::t;
use pnex_core::media_ingest::{MediaStream, MediaStreamInput, MediaStreamKind};

use crate::api;
use crate::components::crud::form::FormDialog;
use crate::components::secret_field::{can_manage_secrets, SecretDraft, SecretField};
use crate::state::toasts;

const INPUT: &str = "w-full px-3 py-2 border border-gray-300 rounded-lg text-sm bg-white";
const LABEL: &str = "block text-xs text-gray-600 mb-1";

/// Retention choices offered by the form (any `days:N` stays valid on
/// the server).
const RETENTIONS: [&str; 5] = ["none", "days:1", "days:7", "days:30", "keep"];

pub(super) fn retention_label(wire: &str) -> String {
    match wire {
        "none" => t!("streams-retention-none").to_string(),
        "keep" => t!("streams-retention-keep").to_string(),
        other => {
            let days = other.trim_start_matches("days:").to_string();
            t!("streams-retention-days", days: days).to_string()
        }
    }
}

#[component]
pub fn StreamFormModal(
    existing: Option<MediaStream>,
    on_close: Callback<()>,
    on_saved: Callback<()>,
) -> Element {
    let init = existing.clone();
    let mut name = use_signal(|| init.as_ref().map(|s| s.name.clone()).unwrap_or_default());
    let mut kind = use_signal(|| init.as_ref().map(|s| s.kind).unwrap_or_default());
    let mut url = use_signal(|| init.as_ref().map(|s| s.url.clone()).unwrap_or_default());
    let mut profile = use_signal(|| {
        init.as_ref()
            .and_then(|s| s.asr_profile_id.clone())
            .unwrap_or_default()
    });
    let mut segment_secs = use_signal(|| init.as_ref().map(|s| s.segment_secs).unwrap_or(30));
    let mut retention = use_signal(|| {
        init.as_ref()
            .map(|s| s.audio_retention.clone())
            .unwrap_or_else(|| "none".into())
    });
    let mut tdm_checked = use_signal(|| init.as_ref().is_some_and(|s| s.tdm_checked_at.is_some()));
    let mut tdm_note = use_signal(|| {
        init.as_ref()
            .map(|s| s.tdm_note.clone())
            .unwrap_or_default()
    });
    let secret =
        use_signal(|| SecretDraft::from_view(init.as_ref().and_then(|s| s.auth_secret.clone())));
    let mut busy = use_signal(|| false);
    let can_manage = can_manage_secrets();

    let profiles = use_resource(|| async move { api::media_streams::profiles().await });
    let profile_rows = match &*profiles.value().read() {
        Some(Ok(rows)) => rows.clone(),
        _ => Vec::new(),
    };

    let editing_id = existing.as_ref().map(|s| s.id.clone());
    let had_secret = existing.as_ref().is_some_and(|s| s.has_secret);
    let was_checked = existing
        .as_ref()
        .is_some_and(|s| s.tdm_checked_at.is_some());
    let save = move |_| {
        let id = editing_id.clone();
        let draft = secret();
        let auth_secret = match &draft {
            SecretDraft::Keep(_) => None,
            other => other.to_input(),
        };
        let input = MediaStreamInput {
            name: Some(name()),
            kind: Some(kind()),
            url: Some(url()),
            auth_secret,
            clear_secret: had_secret && draft == SecretDraft::Empty,
            asr_profile_id: Some(profile()),
            segment_secs: Some(segment_secs()),
            audio_retention: Some(retention()),
            tdm_checked: (tdm_checked() != was_checked).then_some(tdm_checked()),
            tdm_note: Some(tdm_note()),
            ..Default::default()
        };
        busy.set(true);
        spawn(async move {
            let result = match id {
                Some(id) => api::media_streams::update(&id, &input).await,
                None => api::media_streams::create(&input).await,
            };
            busy.set(false);
            match result {
                Ok(_) => {
                    toasts::success(t!("streams-saved").to_string());
                    on_saved.call(());
                }
                Err(err) => toasts::error(err),
            }
        });
    };

    let title = if existing.is_some() {
        t!("streams-edit-title").to_string()
    } else {
        t!("streams-create-title").to_string()
    };
    let valid = !name().trim().is_empty() && !url().trim().is_empty();
    rsx! {
        FormDialog {
            title,
            submit_label: t!("streams-save").to_string(),
            on_close,
            on_submit: save,
            busy: busy(),
            valid,
            max_width: "max-w-xl".to_string(),
            div {
                label { r#for: "stream-name", class: LABEL, {t!("streams-name")} }
                input {
                    id: "stream-name",
                    class: INPUT,
                    r#type: "text",
                    value: "{name}",
                    oninput: move |e| name.set(e.value()),
                }
            }
            div { class: "grid grid-cols-1 sm:grid-cols-3 gap-3",
                div {
                    label { r#for: "stream-kind", class: LABEL, {t!("streams-kind")} }
                    select {
                        id: "stream-kind",
                        class: INPUT,
                        onchange: move |e| {
                            if let Some(k) = MediaStreamKind::from_wire(&e.value()) {
                                kind.set(k);
                            }
                        },
                        for k in MediaStreamKind::ALL {
                            option { value: k.wire(), selected: kind() == k, {k.wire()} }
                        }
                    }
                }
                div { class: "sm:col-span-2",
                    label { r#for: "stream-url", class: LABEL, {t!("streams-url")} }
                    input {
                        id: "stream-url",
                        class: INPUT,
                        r#type: "url",
                        value: "{url}",
                        placeholder: "https://…",
                        oninput: move |e| url.set(e.value()),
                    }
                }
            }
            p { class: "text-xs text-gray-500", {t!("streams-url-help")} }
            SecretField {
                label: t!("streams-secret").to_string(),
                draft: secret,
                can_manage,
            }
            div { class: "grid grid-cols-1 sm:grid-cols-3 gap-3",
                div { class: "sm:col-span-2",
                    label { r#for: "stream-profile", class: LABEL, {t!("streams-profile")} }
                    select {
                        id: "stream-profile",
                        class: INPUT,
                        onchange: move |e| profile.set(e.value()),
                        option { value: "", selected: profile().is_empty(),
                            {t!("streams-profile-none")}
                        }
                        for p in profile_rows {
                            option { value: "{p.id}", selected: profile() == p.id, "{p.name}" }
                        }
                    }
                }
                div {
                    label { r#for: "stream-segment", class: LABEL, {t!("streams-segment-secs")} }
                    input {
                        id: "stream-segment",
                        class: INPUT,
                        r#type: "number",
                        min: "10",
                        max: "120",
                        value: "{segment_secs}",
                        oninput: move |e| {
                            if let Ok(v) = e.value().parse::<i32>() {
                                segment_secs.set(v);
                            }
                        },
                    }
                }
            }
            div {
                label { r#for: "stream-retention", class: LABEL, {t!("streams-retention")} }
                select {
                    id: "stream-retention",
                    class: INPUT,
                    onchange: move |e| retention.set(e.value()),
                    for r in RETENTIONS {
                        option { value: r, selected: retention() == r, {retention_label(r)} }
                    }
                }
                p { class: "text-xs text-gray-500 mt-1", {t!("streams-retention-help")} }
            }
            div { class: "rounded-lg border border-amber-200 bg-amber-50 p-3 space-y-2",
                label { class: "inline-flex items-center gap-2 text-sm text-gray-800",
                    input {
                        r#type: "checkbox",
                        checked: tdm_checked(),
                        onchange: move |e| tdm_checked.set(e.checked()),
                    }
                    {t!("streams-tdm-checked")}
                }
                p { class: "text-xs text-gray-600", {t!("streams-tdm-help")} }
                textarea {
                    class: INPUT,
                    rows: "2",
                    placeholder: t!("streams-tdm-note"),
                    value: "{tdm_note}",
                    oninput: move |e| tdm_note.set(e.value()),
                }
            }
        }
    }
}
