//! `/voice-id` — voice profiles of the org (voice-id.md): which persons are
//! enrolled, their state, and identification of a clip among every ready
//! profile of an embedding model. Enrolment itself lives on the Person
//! object page ("Voice" tab), next to the consent.

use dioxus::prelude::*;
use dioxus_i18n::t;
use pnex_core::ontology::api::{ObjectView, OntologyQuery};
use pnex_core::voice_id::{VoiceProfileView, PERSON_TYPE};

use crate::api;
use crate::app::Route;
use crate::components::crud::layout::ListLayout;
use crate::components::crud::pager::{ListPager, PAGE_SIZE};
use crate::components::crud::states::ListStates;
use crate::components::crud::table::{Column, DataTable, RowKey};
use crate::components::modal::Modal;
use crate::components::ui::{FIELD_LABEL, INPUT};
use crate::components::voice_id::{embedding_models, status_label, VoiceTestBox};

const SECTION: &str = "rounded-lg border border-line bg-surface p-4 space-y-3";

#[component]
pub fn VoiceId() -> Element {
    let mut reload = use_signal(|| 0u32);
    let page = use_signal(|| 0i64);
    let mut picking = use_signal(|| false);
    let navigator = use_navigator();
    let profiles = use_resource(move || async move {
        let _ = reload();
        api::voice_id::list().await
    });
    let models = use_resource(embedding_models);
    let mut model_id = use_signal(String::new);
    let models_loaded = models.read().is_some();
    let list = models.read().clone().unwrap_or_default();
    // Nothing picked yet = the first model (no signal write during render).
    let chosen = match model_id() {
        m if m.is_empty() => list.first().map(|m| m.id.clone()).unwrap_or_default(),
        m => m,
    };
    let (state, all) = match &*profiles.read() {
        None => (None, Vec::new()),
        Some(Ok(v)) => (Some(Ok(())), v.clone()),
        Some(Err(e)) => (Some(Err(e.clone())), Vec::new()),
    };
    let any_ready = all
        .iter()
        .any(|p| p.status == pnex_core::voice_id::VoiceProfileStatus::Ready);
    let profiles_loaded = state.is_some();
    let count = all.len() as i64;
    // shortcut: client-side paging, the API returns every profile of the org; page server-side if orgs enrol hundreds.
    let rows: Vec<VoiceProfileView> = all
        .into_iter()
        .skip((page() * PAGE_SIZE) as usize)
        .take(PAGE_SIZE as usize)
        .collect();
    let is_empty = rows.is_empty();
    let can_identify = models_loaded && !list.is_empty() && any_ready;
    let columns = vec![
        Column::new(t!("voice-col-person").to_string(), |p: &VoiceProfileView| {
            rsx! {
                span { class: "font-medium text-gray-900", "{p.person}" }
            }
        }),
        Column::new(t!("voice-model").to_string(), |p: &VoiceProfileView| {
            let model = p.model_name.clone().unwrap_or_default();
            rsx! {
                span { class: "text-sm text-gray-700", "{model}" }
            }
        })
        .secondary(),
        Column::new(t!("voice-samples").to_string(), |p: &VoiceProfileView| {
            rsx! {
                span { class: "text-sm text-gray-700", {t!("voice-page-clips", count : p.samples.len())} }
            }
        })
        .secondary(),
        Column::new(t!("voice-col-status").to_string(), |p: &VoiceProfileView| {
            rsx! {
                span { class: "rounded bg-gray-100 px-2 py-0.5 text-xs text-ink", {status_label(p.status.wire())} }
            }
        }),
    ];
    rsx! {
        ListLayout {
            title: t!("voice-page-title").to_string(),
            // Enrolment (consent) is owners and admins only.
            can_write: crate::state::access::can("voice-id:write"),
            add_label: Some(t!("voice-new").to_string()),
            on_add: move |_| picking.set(true),
            subtitle: Some(t!("voice-page-subtitle").to_string()),
            on_refresh: move |_| reload.with_mut(|r| *r += 1),
            ListStates {
                state,
                is_empty,
                empty_message: t!("voice-page-empty").to_string(),
                div { class: "relative",
                    DataTable {
                        columns,
                        rows,
                        row_key: RowKey::new(|p: &VoiceProfileView| p.object_id.clone()),
                        on_row_click: move |id: String| {
                            navigator.push(Route::OntologyObject { id });
                        },
                    }
                    ListPager { count, page }
                }
            }
            div { class: "{SECTION} mt-4",
                h3 { class: "text-base font-semibold text-ink", {t!("voice-identify-title")} }
                p { class: "text-sm text-slate", {t!("voice-identify-help")} }
                if models_loaded && list.is_empty() {
                    p { class: "text-sm text-amber-700", {t!("err-voice-id-no-model")} }
                } else if profiles_loaded && !any_ready {
                    p { class: "text-sm text-amber-700", {t!("voice-identify-no-profile")} }
                } else if can_identify {
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
                }
                if can_identify {
                    VoiceTestBox {
                        key: "{chosen}",
                        object_id: None,
                        model_id: Some(chosen.clone()),
                    }
                }
            }
            if picking() {
                PersonPicker { on_close: move |_| picking.set(false) }
            }
        }
    }
}

/// "New voice profile": pick the person, enrolment continues on their Voice tab.
#[component]
fn PersonPicker(on_close: Callback<()>) -> Element {
    let navigator = use_navigator();
    let persons = use_resource(|| async {
        let q = OntologyQuery {
            type_key: Some(PERSON_TYPE.to_string()),
            limit: Some(500),
            ..Default::default()
        };
        api::ontology::query(&q).await
    });
    let rows: Vec<ObjectView> = match &*persons.read() {
        Some(Ok(r)) => r.rows.iter().map(|r| r.object.clone()).collect(),
        _ => Vec::new(),
    };
    let loaded = persons.read().is_some();
    rsx! {
        Modal {
            title: t!("voice-pick-person").to_string(),
            max_width: "max-w-md".to_string(),
            on_close: move |_| on_close.call(()),
            if loaded && rows.is_empty() {
                p { class: "text-sm text-gray-600 mb-3", {t!("voice-pick-person-empty")} }
                button {
                    r#type: "button",
                    class: "px-3 py-2 rounded-lg text-sm bg-signal-600 text-white",
                    onclick: move |_| {
                        on_close.call(());
                        navigator
                            .push(Route::Ontology {
                                tab: String::new(),
                                type_key: String::new(),
                            });
                    },
                    {t!("voice-pick-person-ontology")}
                }
            }
            ul { class: "divide-y divide-line max-h-96 overflow-y-auto",
                for o in rows {
                    PersonPickerItem { key: "{o.id}", person: o }
                }
            }
        }
    }
}

#[component]
fn PersonPickerItem(person: ObjectView) -> Element {
    let navigator = use_navigator();
    let id = person.id.clone();
    rsx! {
        li {
            button {
                r#type: "button",
                class: "w-full text-left px-2 py-2 text-sm hover:bg-gray-50",
                onclick: move |_| {
                    navigator
                        .push(Route::OntologyObject {
                            id: id.clone(),
                        });
                },
                "{person.title}"
            }
        }
    }
}
