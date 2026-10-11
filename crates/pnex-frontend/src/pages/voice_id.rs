//! `/voice-id` — voice profiles of the org (voice-id.md): which persons are
//! enrolled, their state, and identification of a clip among every ready
//! profile of an embedding model. Enrolment itself lives on the Person
//! object page ("Voice" tab), next to the consent.

use dioxus::prelude::*;
use dioxus_i18n::t;

use crate::api;
use crate::app::Route;
use crate::components::crud::layout::ListLayout;
use crate::components::ui::{FIELD_LABEL, INPUT};
use crate::components::voice_id::{embedding_models, status_label, VoiceTestBox};

const SECTION: &str = "rounded-lg border border-line bg-surface p-4 space-y-3";

#[component]
pub fn VoiceId() -> Element {
    let mut reload = use_signal(|| 0u32);
    let profiles = use_resource(move || async move {
        let _ = reload();
        api::voice_id::list().await
    });
    let models = use_resource(embedding_models);
    let mut model_id = use_signal(String::new);
    let list = models.read().clone().unwrap_or_default();
    // Nothing picked yet = the first model (no signal write during render).
    let chosen = match model_id() {
        m if m.is_empty() => list.first().map(|m| m.id.clone()).unwrap_or_default(),
        m => m,
    };
    let rows = match &*profiles.read() {
        Some(Ok(v)) => v.clone(),
        _ => Vec::new(),
    };
    rsx! {
        ListLayout {
            title: t!("voice-page-title").to_string(),
            can_write: crate::state::org::current_can_write(),
            subtitle: Some(t!("voice-page-subtitle").to_string()),
            on_refresh: move |_| reload.with_mut(|r| *r += 1),
            div { class: "space-y-4",
                div { class: SECTION,
                    if rows.is_empty() {
                        p { class: "text-sm text-slate", {t!("voice-page-empty")} }
                    }
                    ul { class: "divide-y divide-line",
                        for p in rows {
                            ProfileRow { key: "{p.id}", profile: p }
                        }
                    }
                }
                div { class: SECTION,
                    h3 { class: "text-base font-semibold text-ink", {t!("voice-identify-title")} }
                    p { class: "text-sm text-slate", {t!("voice-identify-help")} }
                    div { class: "max-w-sm",
                        label { r#for: "voice-identify-model", class: FIELD_LABEL,
                            {t!("voice-model")}
                        }
                        select {
                            id: "voice-identify-model",
                            class: INPUT,
                            onchange: move |e| model_id.set(e.value()),
                            for m in list.iter() {
                                option { value: "{m.id}", selected: chosen == m.id, "{m.name}" }
                            }
                        }
                    }
                    VoiceTestBox {
                        key: "{chosen}",
                        object_id: None,
                        model_id: Some(chosen.clone()),
                    }
                }
            }
        }
    }
}

#[component]
fn ProfileRow(profile: pnex_core::voice_id::VoiceProfileView) -> Element {
    let navigator = use_navigator();
    let object_id = profile.object_id.clone();
    rsx! {
        li { class: "flex flex-wrap items-center gap-3 py-2 text-sm",
            button {
                class: "grow text-left font-medium text-signal hover:underline",
                onclick: move |_| {
                    navigator
                        .push(Route::OntologyObject {
                            id: object_id.clone(),
                        });
                },
                "{profile.person}"
            }
            span { class: "text-slate", {profile.model_name.clone().unwrap_or_default()} }
            span { class: "text-slate", {t!("voice-page-clips", count : profile.samples.len())} }
            span { class: "rounded bg-gray-100 px-2 py-0.5 text-ink", {status_label(&profile.status)} }
        }
    }
}
