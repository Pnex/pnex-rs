//! `/ontology` — the generic explorer of the ontology (ontology.md D186):
//! objects by type (search, property-aware creation), types and link
//! types (the schema), packs and the YAML as-code round trip. The
//! specialised pages (devices, media…) stay; they are views of system
//! types and link here through their object page.

pub mod object;
pub mod packs;
pub mod schema;
pub mod type_editor;

use dioxus::prelude::*;
use dioxus_i18n::t;
use pnex_core::ontology::api::{ObjectInput, ObjectTypeView, ObjectView, OntologyQuery, QueryRow};
use serde_json::{Map, Value};

use crate::api;
use crate::app::Route;
use crate::components::crud::filters::{FilterBar, SearchInput};
use crate::components::crud::form::FormDialog;
use crate::components::crud::layout::ListLayout;
use crate::components::crud::pager::{ListPager, PAGE_SIZE};
use crate::components::crud::states::ListStates;
use crate::components::crud::table::{Column, DataTable, RowKey};
use crate::components::ontology::property_input::PropertyInput;
use crate::components::ontology::{short_time, type_label, type_label_of};
use crate::state::{org, toasts};

pub use object::OntologyObject;
pub use type_editor::OntologyType;

const TAB_ACTIVE: &str = "px-3 py-1.5 text-sm font-medium rounded-lg bg-blue-600 text-white";
const TAB_IDLE: &str = "px-3 py-1.5 text-sm font-medium rounded-lg text-gray-600 hover:bg-gray-100";

#[component]
pub fn Ontology(tab: String, type_key: String) -> Element {
    let tab = if tab.is_empty() {
        "objects".to_string()
    } else {
        tab
    };
    let navigator = use_navigator();
    let tabs = [
        ("objects", t!("onto-tab-objects")),
        ("schema", t!("onto-tab-schema")),
        ("packs", t!("onto-tab-packs")),
    ];
    rsx! {
        div { class: "space-y-4",
            div { class: "flex flex-wrap gap-2", role: "tablist",
                for (key, label) in tabs {
                    button {
                        key: "{key}",
                        r#type: "button",
                        role: "tab",
                        "data-tab": "{key}",
                        class: if tab == key { TAB_ACTIVE } else { TAB_IDLE },
                        onclick: move |_| {
                            navigator
                                .push(Route::Ontology {
                                    tab: key.to_string(),
                                    type_key: String::new(),
                                });
                        },
                        "{label}"
                    }
                }
            }
            if org::current().is_none() {
                p { class: "text-gray-500 text-center py-12", {t!("orgs-empty")} }
            } else if tab == "schema" {
                schema::SchemaTab {}
            } else if tab == "packs" {
                packs::PacksTab {}
            } else {
                ObjectsTab { initial_type: type_key }
            }
        }
    }
}

#[component]
fn ObjectsTab(initial_type: String) -> Element {
    let mut reload = use_signal(|| 0u32);
    let mut type_key = use_signal(move || initial_type.clone());
    let search = use_signal(String::new);
    let mut page = use_signal(|| 0i64);
    let mut creating = use_signal(|| false);
    let can_write = org::current_can_write();
    let navigator = use_navigator();

    let types = use_resource(move || async move { api::ontology::types().await });
    let list = use_resource(move || {
        let q = OntologyQuery {
            type_key: Some(type_key()).filter(|t| !t.is_empty()),
            text: Some(search().trim().to_string()).filter(|t| !t.is_empty()),
            limit: Some(PAGE_SIZE as u64),
            offset: Some((page() * PAGE_SIZE) as u64),
            ..Default::default()
        };
        async move {
            let _ = reload();
            api::ontology::query(&q).await
        }
    });
    let all_types: Vec<ObjectTypeView> = types
        .read()
        .as_ref()
        .and_then(|r| r.as_ref().ok())
        .cloned()
        .unwrap_or_default();
    let (state, is_empty, count, rows) = match &*list.read() {
        None => (None, false, 0, Vec::new()),
        Some(Ok(r)) => (Some(Ok(())), r.rows.is_empty(), r.count, r.rows.clone()),
        Some(Err(e)) => (Some(Err(e.clone())), false, 0, Vec::new()),
    };
    let selected = all_types.iter().find(|t| t.def.key == type_key()).cloned();
    let can_create = can_write && selected.as_ref().is_some_and(|t| !t.def.system);
    let label_types = all_types.clone();
    let columns = vec![
        Column::new(t!("onto-col-title").to_string(), |r: &QueryRow| {
            rsx! {
                span { class: "font-medium text-gray-900", "{r.object.title}" }
            }
        }),
        Column::new(t!("onto-col-type").to_string(), move |r: &QueryRow| {
            let label = type_label_of(&label_types, &r.object.type_key);
            rsx! {
                span { class: "text-sm text-gray-700", "{label}" }
            }
        }),
        Column::new(t!("onto-col-updated").to_string(), |r: &QueryRow| {
            let at = short_time(&r.object.updated_at);
            rsx! {
                span { class: "text-xs text-gray-500", "{at}" }
            }
        })
        .secondary(),
    ];

    rsx! {
        ListLayout {
            title: t!("nav-ontology").to_string(),
            subtitle: Some(t!("onto-subtitle").to_string()),
            can_write: can_create,
            add_label: Some(t!("onto-new-object").to_string()),
            on_add: move |_| creating.set(true),
            on_refresh: move |_| reload.with_mut(|r| *r += 1),
            FilterBar {
                select {
                    class: "px-3 py-2 border border-gray-300 rounded-lg text-sm bg-white",
                    "data-testid": "onto-type-filter",
                    onchange: move |e| {
                        type_key.set(e.value());
                        page.set(0);
                    },
                    option { value: "", selected: type_key().is_empty(), {t!("onto-all-types")} }
                    for t in all_types.iter() {
                        option {
                            key: "{t.def.key}",
                            value: "{t.def.key}",
                            selected: t.def.key == type_key(),
                            {type_label(&t.def)}
                        }
                    }
                }
                SearchInput {
                    placeholder: t!("onto-search").to_string(),
                    value: search,
                    on_submit: move |_| {
                        page.set(0);
                        reload.with_mut(|r| *r += 1);
                    },
                }
            }
            ListStates {
                state,
                is_empty,
                empty_message: t!("onto-objects-empty").to_string(),
                div { class: "relative",
                    DataTable {
                        columns,
                        rows,
                        row_key: RowKey::new(|r: &QueryRow| r.object.id.clone()),
                        on_row_click: move |id: String| {
                            navigator.push(Route::OntologyObject { id });
                        },
                    }
                    ListPager { count, page }
                }
            }
            if creating() {
                if let Some(t) = selected.clone() {
                    ObjectForm {
                        type_view: t,
                        initial: None,
                        on_close: move |_| creating.set(false),
                        on_saved: move |o: ObjectView| {
                            creating.set(false);
                            navigator.push(Route::OntologyObject { id: o.id });
                        },
                    }
                }
            }
        }
    }
}

/// Create (`initial: None`) or edit an object; for a system object only
/// the org's extra properties are editable.
#[component]
pub fn ObjectForm(
    type_view: ObjectTypeView,
    initial: Option<ObjectView>,
    on_close: Callback<()>,
    on_saved: Callback<ObjectView>,
) -> Element {
    let start = initial.clone();
    let mut title = use_signal(move || start.as_ref().map(|o| o.title.clone()).unwrap_or_default());
    let start = initial.clone();
    let doc = use_signal(move || {
        start
            .as_ref()
            .map(|o| o.properties.clone())
            .unwrap_or_default()
    });
    let mut busy = use_signal(|| false);
    let mut errors = use_signal(Map::<String, Value>::new);
    let def = type_view.def.clone();
    let system = def.system;
    // `ref` choices: the live objects of each referenced type.
    let ref_types: Vec<String> = def
        .properties
        .iter()
        .filter_map(|p| match &p.kind {
            pnex_core::ontology::PropertyKind::Ref { to_type } => Some(to_type.clone()),
            _ => None,
        })
        .collect();
    let refs = use_resource(move || {
        let ref_types = ref_types.clone();
        async move {
            let mut out: std::collections::BTreeMap<String, Vec<(String, String)>> =
                Default::default();
            for t in ref_types {
                let q = OntologyQuery {
                    type_key: Some(t.clone()),
                    limit: Some(500),
                    ..Default::default()
                };
                if let Ok(r) = api::ontology::query(&q).await {
                    out.insert(
                        t,
                        r.rows
                            .into_iter()
                            .map(|r| (r.object.id, r.object.title))
                            .collect(),
                    );
                }
            }
            out
        }
    });
    let refs = refs.read().clone().unwrap_or_default();
    let props: Vec<_> = def
        .properties
        .iter()
        .filter(|p| !p.kind.is_temporal())
        .cloned()
        .collect();
    let title_label = t!("onto-title");
    let heading = if initial.is_some() {
        t!("onto-edit-object")
    } else {
        t!("onto-new-typed", kind : type_label(& def))
    };
    let local_check = move || {
        let props_def = def.properties.clone();
        pnex_core::ontology::schema::check_values(&props_def, &doc.read())
    };

    rsx! {
        FormDialog {
            title: heading.to_string(),
            submit_label: t!("common-save").to_string(),
            busy: busy(),
            valid: system || !title().trim().is_empty(),
            max_width: "max-w-xl".to_string(),
            on_close: move |_| on_close.call(()),
            on_submit: move |_| {
                // Same validation as the server, before the round trip.
                if let Err((f, tok)) = local_check() {
                    errors.set(Map::from_iter([(f, Value::String(tok))]));
                    return;
                }
                let input = ObjectInput {
                    type_key: type_view.def.key.clone(),
                    title: title().trim().to_string(),
                    properties: doc.read().clone(),
                    expected_version: initial.as_ref().map(|o| o.version),
                };
                let id = initial.as_ref().map(|o| o.id.clone());
                busy.set(true);
                spawn(async move {
                    let res = match id {
                        Some(id) => api::ontology::update_object(&id, &input).await,
                        None => api::ontology::create_object(&input).await,
                    };
                    busy.set(false);
                    match res {
                        Ok(o) => on_saved.call(o),
                        Err(e) => {
                            if let Some(fields) = crate::components::ontology::field_errors(&e) {
                                errors.set(fields);
                            } else {
                                toasts::error(e);
                            }
                        }
                    }
                });
            },
            div { class: "space-y-3",
                if !system {
                    div {
                        label { class: "block text-xs font-medium text-gray-500 uppercase mb-1",
                            "{title_label} *"
                        }
                        input {
                            class: "w-full px-3 py-2 border border-gray-300 rounded-lg text-sm",
                            "data-testid": "onto-object-title",
                            value: "{title}",
                            oninput: move |e| title.set(e.value()),
                        }
                    }
                }
                for p in props {
                    PropertyInput {
                        key: "{p.key}",
                        refs: match &p.kind {
                            pnex_core::ontology::PropertyKind::Ref { to_type } => {
                                refs.get(to_type).cloned().unwrap_or_default()
                            }
                            _ => Vec::new(),
                        },
                        error: errors
                            .read()
                            .get(&p.key)
                            .and_then(|v| v.as_str())
                            .map(|t| crate::api::error_i18n::localize_field(&p.key, t)),
                        def: p.clone(),
                        doc,
                    }
                }
            }
        }
    }
}
