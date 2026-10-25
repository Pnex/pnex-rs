//! Schema tab (object types, system and org, with their object counts) and
//! link types tab. Editing the schema is for org owners and admins (D188); the
//! server enforces it, the UI only hides the buttons.

use dioxus::prelude::*;
use dioxus_i18n::t;
use pnex_core::ontology::api::{LinkTypeView, ObjectTypeView};
use pnex_core::ontology::{LinkTypeDef, PropertyKind, TypeSet};
use serde_json::{Map, Value};

use crate::api;
use crate::app::Route;
use crate::components::confirm::ConfirmDialog;
use crate::components::crud::form::FormDialog;
use crate::components::crud::layout::{ListLayout, DANGER_BTN};
use crate::components::crud::pager::{page_slice, ListPager};
use crate::components::crud::states::ListStates;
use crate::components::crud::table::{Column, DataTable, RowKey};
use crate::components::ontology::{
    field_errors, icon_of, link_label, type_label, type_label_of, TypeIcon,
};
use crate::state::toasts;

use super::type_editor::EnumDisplay;

const INPUT: &str = crate::components::ui::INPUT;
const LABEL: &str = crate::components::ui::FIELD_LABEL;

/// Names of a set of types: org types by their (translated) name, system
/// types by their key's translation.
fn set_label(set: &TypeSet, types: &[ObjectTypeView]) -> String {
    match set {
        TypeSet::Any => t!("onto-any-type").to_string(),
        TypeSet::Only(keys) => keys
            .iter()
            .map(|k| type_label_of(types, k))
            .collect::<Vec<_>>()
            .join(", "),
    }
}

fn schema_rank(v: &ObjectTypeView) -> u8 {
    match (v.def.system, &v.pack_key) {
        (true, _) => 2,
        (false, Some(_)) => 1,
        (false, None) => 0,
    }
}

#[component]
pub fn SchemaTab(tabs: Element) -> Element {
    let mut reload = use_signal(|| 0u32);
    let navigator = use_navigator();
    let admin = crate::state::access::can("ontology.schema:write");
    let types = use_resource(move || async move {
        let _ = reload();
        api::ontology::types().await
    });
    let (state, mut rows) = match &*types.read() {
        None => (None, Vec::new()),
        Some(Ok(r)) => (Some(Ok(())), r.clone()),
        Some(Err(e)) => (Some(Err(e.clone())), Vec::new()),
    };
    // The org's own types first, then pack types, system types last.
    rows.sort_by_key(schema_rank);
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
    let type_page = use_signal(|| 0i64);
    let type_count = rows.len() as i64;
    let rows = page_slice(&rows, type_page());
    rsx! {
        ListLayout {
            title: t!("onto-types").to_string(),
            tabs,
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
                is_empty: type_count == 0,
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
                ListPager { count: type_count, page: type_page }
            }
        }
    }
}

/// Link types tab: its own list (one list per view, D224), the add button
/// in the header like every list.
#[component]
pub fn LinkTypesTab(tabs: Element) -> Element {
    let mut reload = use_signal(|| 0u32);
    let admin = crate::state::access::can("ontology.schema:write");
    let links = use_resource(move || async move {
        let _ = reload();
        api::ontology::link_types().await
    });
    // The new link form picks its ends among the types.
    let types = use_resource(move || async move {
        let _ = reload();
        api::ontology::types().await.unwrap_or_default()
    });
    let (state, all) = match &*links.read() {
        None => (None, Vec::new()),
        Some(Ok(r)) => (Some(Ok(())), r.clone()),
        Some(Err(e)) => (Some(Err(e.clone())), Vec::new()),
    };
    let page = use_signal(|| 0i64);
    let count = all.len() as i64;
    let rows = page_slice(&all, page());
    let mut delete_link = use_signal(|| None::<String>);
    let mut new_link = use_signal(|| false);
    let mut shown = use_signal(|| None::<LinkTypeView>);
    let known: Vec<ObjectTypeView> = types.read().clone().unwrap_or_default();
    let columns = vec![
        Column::new(t!("onto-col-type").to_string(), |l: &LinkTypeView| {
            rsx! {
                span { class: "font-medium text-gray-900", {link_label(Some(&l.def), &l.def.key, false)} }
                code { class: "ml-2 text-xs text-gray-500", "{l.def.key}" }
            }
        }),
        Column::new(t!("onto-col-ends").to_string(), move |l: &LinkTypeView| {
            let ends = format!(
                "{} → {}",
                set_label(&l.def.from_types, &known),
                set_label(&l.def.to_types, &known),
            );
            rsx! {
                span { class: "text-xs text-gray-600", "{ends}" }
            }
        })
        .secondary(),
        Column::new(t!("common-actions").to_string(), move |l: &LinkTypeView| {
            let key = l.def.key.clone();
            let system = l.def.system;
            let has_enum = l
                .def
                .attributes
                .iter()
                .any(|a| matches!(a.kind, PropertyKind::Enum { .. }));
            let view = l.clone();
            rsx! {
                if system {
                    span { class: "rounded bg-gray-100 px-2 py-0.5 text-xs text-gray-600", {t!("onto-system")} }
                } else if admin {
                    if has_enum {
                        button {
                            class: "text-xs text-gray-700 hover:underline",
                            "data-testid": "onto-link-values",
                            onclick: move |_| shown.set(Some(view.clone())),
                            {t!("onto-link-values")}
                        }
                    }
                    DeleteButton { value: key, on_delete: move |k| delete_link.set(Some(k)) }
                }
            }
        })
        .actions(),
    ];
    rsx! {
        ListLayout {
            title: t!("onto-link-types").to_string(),
            tabs,
            subtitle: Some(t!("onto-link-types-subtitle").to_string()),
            can_write: admin,
            add_label: Some(t!("onto-new-link-type").to_string()),
            on_add: move |_| new_link.set(true),
            on_refresh: move |_| reload.with_mut(|r| *r += 1),
            ListStates {
                state,
                is_empty: count == 0,
                empty_message: String::new(),
                DataTable {
                    columns,
                    rows,
                    row_key: RowKey::new(|l: &LinkTypeView| l.def.key.clone()),
                }
                ListPager { count, page }
            }
        }
        if new_link() {
            LinkTypeForm {
                types: types.read().clone().unwrap_or_default(),
                on_close: move |_| new_link.set(false),
                on_saved: move |_| {
                    new_link.set(false);
                    reload.with_mut(|r| *r += 1);
                },
            }
        }
        if let Some(view) = shown() {
            LinkValuesForm {
                view,
                on_close: move |_| shown.set(None),
                on_saved: move |_| {
                    shown.set(None);
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
    let mut evidence = use_signal(|| false);
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
            inverse_names: Default::default(),
            names: Default::default(),
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
            evidence: evidence(),
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
                label { class: "flex items-center gap-2 text-sm text-gray-700",
                    input {
                        r#type: "checkbox",
                        checked: evidence(),
                        onchange: move |e| evidence.set(e.checked()),
                    }
                    {t!("onto-evidence")}
                }
            }
        }
    }
}

/// How the values of a link type's enum attributes are shown (D370):
/// labels and colours, the values themselves unchanged.
#[component]
fn LinkValuesForm(view: LinkTypeView, on_close: Callback<()>, on_saved: Callback<()>) -> Element {
    let mut def = use_signal(|| view.def.clone());
    let mut busy = use_signal(|| false);
    let version = view.version;
    let on_submit = move |_| {
        let input = pnex_core::ontology::api::LinkTypeInput {
            def: def(),
            expected_version: Some(version),
        };
        busy.set(true);
        spawn(async move {
            let res = api::ontology::update_link_type(&input.def.key, &input).await;
            busy.set(false);
            match res {
                Ok(_) => on_saved.call(()),
                Err(e) => toasts::error(e),
            }
        });
    };
    let attrs: Vec<(usize, pnex_core::ontology::PropertyDef)> = def
        .read()
        .attributes
        .iter()
        .cloned()
        .enumerate()
        .filter(|(_, a)| matches!(a.kind, PropertyKind::Enum { .. }))
        .collect();
    rsx! {
        FormDialog {
            title: format!(
                "{} — {}",
                t!("onto-link-values"),
                link_label(Some(&view.def), &view.def.key, false),
            ),
            submit_label: t!("common-save").to_string(),
            busy: busy(),
            valid: true,
            on_close: move |_| on_close.call(()),
            on_submit,
            div { class: "space-y-4",
                for (i, a) in attrs {
                    div { key: "{a.key}", class: "space-y-1",
                        p { class: LABEL, {crate::components::ontology::tr(&a.name, &a.names)} }
                        EnumDisplay {
                            prop: a,
                            on_change: move |p| {
                                def.with_mut(|d| {
                                    if let Some(slot) = d.attributes.get_mut(i) {
                                        *slot = p;
                                    }
                                })
                            },
                        }
                    }
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
