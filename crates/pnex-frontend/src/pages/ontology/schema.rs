//! Schema tab: object types (system and org, with their object counts) and
//! link types. Editing the schema is for org owners and admins (D188); the
//! server enforces it, the UI only hides the buttons.

use dioxus::prelude::*;
use dioxus_i18n::t;
use pnex_core::ontology::api::{LinkTypeView, ObjectTypeView};
use pnex_core::ontology::{LinkTypeDef, TypeSet};
use serde_json::{Map, Value};

use crate::api;
use crate::app::Route;
use crate::components::confirm::ConfirmDialog;
use crate::components::crud::form::FormDialog;
use crate::components::crud::layout::{ListLayout, DANGER_BTN};
use crate::components::crud::states::ListStates;
use crate::components::crud::table::{Column, DataTable, RowKey};
use crate::components::ontology::{
    field_errors, icon_of, key_label, link_label, type_label, TypeIcon,
};
use crate::state::{org, toasts};

const INPUT: &str = "w-full px-3 py-2 border border-gray-300 rounded-lg text-sm bg-white";
const LABEL: &str = "block text-xs font-medium text-gray-500 uppercase mb-1";

fn set_label(set: &TypeSet) -> String {
    match set {
        TypeSet::Any => t!("onto-any-type").to_string(),
        TypeSet::Only(keys) => keys
            .iter()
            .map(|k| key_label(k))
            .collect::<Vec<_>>()
            .join(", "),
    }
}

#[component]
pub fn SchemaTab() -> Element {
    let mut reload = use_signal(|| 0u32);
    let navigator = use_navigator();
    let admin = org::current_can_administer();
    let types = use_resource(move || async move {
        let _ = reload();
        api::ontology::types().await
    });
    let links = use_resource(move || async move {
        let _ = reload();
        api::ontology::link_types().await
    });
    let (state, rows) = match &*types.read() {
        None => (None, Vec::new()),
        Some(Ok(r)) => (Some(Ok(())), r.clone()),
        Some(Err(e)) => (Some(Err(e.clone())), Vec::new()),
    };
    let link_rows: Vec<LinkTypeView> = links
        .read()
        .as_ref()
        .and_then(|r| r.as_ref().ok())
        .cloned()
        .unwrap_or_default();
    let columns = vec![
        Column::new(t!("onto-col-type").to_string(), |v: &ObjectTypeView| {
            let icon = icon_of(&v.def);
            let label = type_label(&v.def);
            rsx! {
                div { class: "flex items-center gap-2",
                    TypeIcon { icon, class: "h-4 w-4 text-gray-500" }
                    span { class: "font-medium text-gray-900", "{label}" }
                    code { class: "text-xs text-gray-500", "{v.def.key}" }
                }
            }
        }),
        Column::new(t!("onto-col-origin").to_string(), |v: &ObjectTypeView| {
            let origin = if v.def.system {
                t!("onto-system").to_string()
            } else {
                v.pack_key
                    .clone()
                    .map(|p| t!("onto-from-pack", pack : p).to_string())
                    .unwrap_or_else(|| t!("onto-org-type").to_string())
            };
            rsx! {
                span { class: "text-xs text-gray-600", "{origin}" }
            }
        })
        .secondary(),
        Column::new(t!("onto-col-objects").to_string(), |v: &ObjectTypeView| {
            rsx! {
                span { class: "font-mono text-sm", "{v.object_count}" }
            }
        }),
        Column::new(t!("onto-col-version").to_string(), |v: &ObjectTypeView| {
            rsx! {
                span { class: "text-xs text-gray-500", "v{v.version}" }
            }
        })
        .secondary(),
    ];
    let mut new_link = use_signal(|| false);
    let mut delete_link = use_signal(|| None::<String>);
    let type_rows = rows.clone();
    rsx! {
        ListLayout {
            title: t!("onto-types").to_string(),
            subtitle: Some(t!("onto-types-subtitle").to_string()),
            can_write: admin,
            add_label: Some(t!("onto-new-type").to_string()),
            on_add: move |_| {
                navigator
                    .push(Route::OntologyType {
                        type_key: "_new".into(),
                    });
            },
            on_refresh: move |_| reload.with_mut(|r| *r += 1),
            ListStates {
                state,
                is_empty: rows.is_empty(),
                empty_message: String::new(),
                DataTable {
                    columns,
                    rows,
                    row_key: RowKey::new(|v: &ObjectTypeView| v.def.key.clone()),
                    on_row_click: move |key: String| {
                        navigator
                            .push(Route::OntologyType {
                                type_key: key,
                            });
                    },
                }
            }
        }
        div { class: "mt-6 space-y-3",
            div { class: "flex items-center",
                h2 { class: "text-lg font-semibold text-gray-900", {t!("onto-link-types")} }
                if admin {
                    button {
                        class: "ml-auto px-3 py-1 text-sm text-gray-700 border border-gray-300 rounded-lg hover:bg-gray-50",
                        "data-testid": "onto-new-link-type",
                        onclick: move |_| new_link.set(true),
                        {t!("onto-new-link-type")}
                    }
                }
            }
            ul { class: "divide-y divide-gray-100 rounded-lg border border-gray-200 bg-white",
                for l in link_rows {
                    li {
                        key: "{l.def.key}",
                        class: "flex flex-wrap items-center gap-2 px-4 py-2 text-sm",
                        span { class: "font-medium text-gray-900",
                            {link_label(Some(&l.def), &l.def.key, false)}
                        }
                        code { class: "text-xs text-gray-500", "{l.def.key}" }
                        span { class: "text-xs text-gray-600",
                            {format!("{} → {}", set_label(&l.def.from_types), set_label(&l.def.to_types))}
                        }
                        if l.def.system {
                            span { class: "ml-auto rounded bg-gray-100 px-2 py-0.5 text-xs text-gray-600",
                                {t!("onto-system")}
                            }
                        } else if admin {
                            DeleteButton {
                                value: l.def.key.clone(),
                                on_delete: move |k| delete_link.set(Some(k)),
                            }
                        }
                    }
                }
            }
        }
        if new_link() {
            LinkTypeForm {
                types: type_rows.clone(),
                on_close: move |_| new_link.set(false),
                on_saved: move |_| {
                    new_link.set(false);
                    reload.with_mut(|r| *r += 1);
                },
            }
        }
        if let Some(key) = delete_link() {
            ConfirmDialog {
                title: t!("onto-delete-link-type").to_string(),
                message: t!("onto-delete-link-type-confirm", key : key.clone()).to_string(),
                confirm_label: t!("common-delete").to_string(),
                on_confirm: move |_| {
                    let key = key.clone();
                    spawn(async move {
                        match api::ontology::delete_link_type(&key).await {
                            Ok(_) => reload.with_mut(|r| *r += 1),
                            Err(e) => toasts::error(e),
                        }
                        delete_link.set(None);
                    });
                },
                on_cancel: move |_| delete_link.set(None),
            }
        }
    }
}

/// New org link type: from one type to one type or any (the YAML import
/// covers richer sets).
#[component]
fn LinkTypeForm(
    types: Vec<ObjectTypeView>,
    on_close: Callback<()>,
    on_saved: Callback<()>,
) -> Element {
    let mut key = use_signal(String::new);
    let mut name = use_signal(String::new);
    let mut inverse = use_signal(String::new);
    let mut from = use_signal(String::new);
    let mut to = use_signal(String::new);
    let mut one_target = use_signal(|| false);
    let mut one_source = use_signal(|| false);
    let mut busy = use_signal(|| false);
    let mut errors = use_signal(Map::<String, Value>::new);
    let err = move |f: &str| {
        errors
            .read()
            .get(f)
            .and_then(|v| v.as_str())
            .map(|t| crate::api::error_i18n::localize_field(f, t))
    };
    let choices: Vec<(String, String)> = types
        .iter()
        .map(|t| (t.def.key.clone(), type_label(&t.def)))
        .collect();
    let choices_to = choices.clone();
    let on_submit = move |_| {
        let def = LinkTypeDef {
            key: key().trim().to_string(),
            name: name().trim().to_string(),
            inverse_name: inverse().trim().to_string(),
            system: false,
            from_types: TypeSet::Only(vec![from()]),
            to_types: if to().is_empty() {
                TypeSet::Any
            } else {
                TypeSet::Only(vec![to()])
            },
            one_target: one_target(),
            one_source: one_source(),
            attributes: Vec::new(),
        };
        busy.set(true);
        spawn(async move {
            let res = api::ontology::create_link_type(&def).await;
            busy.set(false);
            match res {
                Ok(_) => on_saved.call(()),
                Err(e) => match field_errors(&e) {
                    Some(f) => errors.set(f),
                    None => toasts::error(e),
                },
            }
        });
    };
    rsx! {
        FormDialog {
            title: t!("onto-new-link-type").to_string(),
            submit_label: t!("common-create").to_string(),
            busy: busy(),
            valid: !key().is_empty() && !name().is_empty() && !from().is_empty(),
            on_close: move |_| on_close.call(()),
            on_submit,
            div { class: "space-y-3",
                div {
                    label { class: LABEL, {t!("onto-key")} }
                    input {
                        class: INPUT,
                        value: "{key}",
                        oninput: move |e| key.set(e.value()),
                    }
                    if let Some(e) = err("key") {
                        p { class: "mt-1 text-xs text-red-600", "{e}" }
                    }
                }
                div {
                    label { class: LABEL, {t!("onto-link-name")} }
                    input {
                        class: INPUT,
                        value: "{name}",
                        oninput: move |e| name.set(e.value()),
                    }
                }
                div {
                    label { class: LABEL, {t!("onto-link-inverse")} }
                    input {
                        class: INPUT,
                        value: "{inverse}",
                        oninput: move |e| inverse.set(e.value()),
                    }
                }
                div { class: "grid grid-cols-2 gap-3",
                    div {
                        label { class: LABEL, {t!("onto-from-type")} }
                        select {
                            class: INPUT,
                            onchange: move |e| from.set(e.value()),
                            option { value: "", "—" }
                            for (k, l) in choices {
                                option { key: "{k}", value: "{k}", "{l}" }
                            }
                        }
                    }
                    div {
                        label { class: LABEL, {t!("onto-to-type")} }
                        select {
                            class: INPUT,
                            onchange: move |e| to.set(e.value()),
                            option { value: "", {t!("onto-any-type")} }
                            for (k, l) in choices_to {
                                option { key: "{k}", value: "{k}", "{l}" }
                            }
                        }
                    }
                }
                label { class: "flex items-center gap-2 text-sm text-gray-700",
                    input {
                        r#type: "checkbox",
                        checked: one_target(),
                        onchange: move |e| one_target.set(e.checked()),
                    }
                    {t!("onto-one-target")}
                }
                label { class: "flex items-center gap-2 text-sm text-gray-700",
                    input {
                        r#type: "checkbox",
                        checked: one_source(),
                        onchange: move |e| one_source.set(e.checked()),
                    }
                    {t!("onto-one-source")}
                }
            }
        }
    }
}

#[component]
fn DeleteButton(value: String, on_delete: Callback<String>) -> Element {
    rsx! {
        button {
            class: "ml-auto {DANGER_BTN}",
            onclick: move |_| on_delete.call(value.clone()),
            {t!("common-delete")}
        }
    }
}
