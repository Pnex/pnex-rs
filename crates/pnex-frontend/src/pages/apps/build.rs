//! The builder of an application (app-builder.md D507–D509): its screens,
//! variables and object sets. A screen's content is edited in the dashboard
//! editor; publishing is what readers see (Q-B2). The scope and the
//! permitted actions stay on the Access page (Q-B1).

use std::collections::BTreeMap;

use dioxus::prelude::*;
use dioxus_i18n::t;
use pnex_core::apps::{
    AppBuildInput, AppScreenInput, AppVariable, AppView, ObjectSetDef, VariableKind,
};
use pnex_core::ontology::api::{FilterOp, ObjectTypeView, OntologyQuery, PropertyFilter};

use super::use_app_by_key;
use crate::api;
use crate::app::Route;
use crate::components::crud::layout::ListLayout;
use crate::components::gerbe::Gerbe;
use crate::components::ontology::filter_row::FilterRow;
use crate::components::ontology::type_label;
use crate::components::ui::{BTN_PRIMARY, BTN_SECONDARY_SM, FIELD_HELP, FIELD_LABEL, INPUT, PANEL};
use crate::state::toasts;

const SECTION: &str = "space-y-3 p-4";
const ROW: &str = "flex flex-wrap items-center gap-2";

#[component]
pub fn AppBuild(app: String) -> Element {
    let found = use_app_by_key(app);
    let state = found.read().clone();
    match state {
        None => rsx! {
            div { class: "flex justify-center py-12",
                Gerbe { size: 28 }
            }
        },
        Some(None) => rsx! {
            p { class: "text-gray-500 text-center py-12", {t!("err-app-not-found")} }
        },
        Some(Some(a)) => rsx! {
            Builder { key: "{a.id}", start: a.clone() }
        },
    }
}

fn input_of(a: &AppView) -> AppBuildInput {
    AppBuildInput {
        screens: a
            .screens
            .iter()
            .map(|s| AppScreenInput {
                key: s.key.clone(),
                name: s.name.clone(),
                icon: s.icon.clone(),
            })
            .collect(),
        home: a.home.clone(),
        variables: a.variables.clone(),
        object_sets: a.object_sets.clone(),
        expected_version: a.version,
    }
}

/// A key free among `taken`: `<prefix>_1`, `<prefix>_2`…
fn free_key<'a>(prefix: &str, taken: impl Iterator<Item = &'a str> + Clone) -> String {
    (1..)
        .map(|n| format!("{prefix}_{n}"))
        .find(|k| !taken.clone().any(|t| t == k))
        .unwrap_or_default()
}

#[component]
fn Builder(start: AppView) -> Element {
    let navigator = use_navigator();
    let first = start.clone();
    let mut saved = use_signal(move || first);
    let first = input_of(&start);
    let mut draft = use_signal(move || first);
    let mut busy = use_signal(|| false);
    let types = use_resource(|| async { api::ontology::types().await.unwrap_or_default() });
    let all_types: Vec<ObjectTypeView> = types
        .read()
        .clone()
        .unwrap_or_default()
        .into_iter()
        .filter(|t| saved.read().scope.types.contains(&t.def.key))
        .collect();
    let dirty = *draft.read() != input_of(&saved.read());
    let current = draft.read().clone();
    let published = saved.read().clone();
    let title = published.name.clone();
    let app_id = published.id.clone();
    let publish_id = published.id.clone();
    let version = published.version;
    let open = super::home_of(&published);
    let run_key = published.key.clone();

    let save = move |_| {
        let id = app_id.clone();
        let input = draft.read().clone();
        let done = t!("apps-saved").to_string();
        busy.set(true);
        spawn(async move {
            match api::apps::build(&id, &input).await {
                Ok(a) => {
                    draft.set(input_of(&a));
                    saved.set(a);
                    toasts::success(done);
                }
                Err(e) => toasts::error(e),
            }
            busy.set(false);
        });
    };
    let publish = move |_| {
        let id = publish_id.clone();
        let done = t!("apps-published").to_string();
        busy.set(true);
        spawn(async move {
            match api::apps::publish(&id, version).await {
                Ok(a) => {
                    draft.set(input_of(&a));
                    saved.set(a);
                    toasts::success(done);
                }
                Err(e) => toasts::error(e),
            }
            busy.set(false);
        });
    };
    let screens_on_server = published.screens.clone();

    rsx! {
        ListLayout {
            title,
            subtitle: Some(t!("apps-build-subtitle").to_string()),
            can_write: true,
            on_back: move |_| {
                navigator.push(Route::Apps {});
            },
            actions: rsx! {
                if let Some(screen) = open {
                    Link {
                        class: BTN_SECONDARY_SM,
                        to: Route::AppRun {
                            app: run_key,
                            screen,
                            query: String::new(),
                        },
                        {t!("apps-open")}
                    }
                }
                button {
                    class: BTN_SECONDARY_SM,
                    "data-testid": "app-publish",
                    disabled: busy() || dirty,
                    title: if dirty { t!("apps-publish-save-first").to_string() } else { String::new() },
                    onclick: publish,
                    {t!("apps-publish")}
                }
                button {
                    class: BTN_PRIMARY,
                    "data-testid": "app-save",
                    disabled: busy() || !dirty,
                    onclick: save,
                    {t!("common-save")}
                }
            },
            div { class: "space-y-4",
                ScreensPanel { draft, screens_on_server }
                VariablesPanel { draft, types: all_types.clone() }
                SetsPanel { draft, types: all_types }
                if !current.screens.is_empty() {
                    p { class: FIELD_HELP, {t!("apps-publish-help")} }
                }
            }
        }
    }
}

#[component]
fn ScreensPanel(
    draft: Signal<AppBuildInput>,
    screens_on_server: Vec<pnex_core::apps::AppScreen>,
) -> Element {
    let navigator = use_navigator();
    let current = draft.read().clone();
    let count = current.screens.len();
    let home = current.home.clone().unwrap_or_default();
    rsx! {
        section { class: "{PANEL} {SECTION}", "data-testid": "app-screens",
            h2 { class: "text-sm font-semibold text-ink", {t!("apps-screens")} }
            for (i, s) in current.screens.iter().cloned().enumerate() {
                div { key: "{s.key}", class: ROW,
                    input {
                        class: "{INPUT} max-w-xs",
                        "data-screen-name": "{s.key}",
                        value: "{s.name}",
                        oninput: move |e| draft.with_mut(|d| d.screens[i].name = e.value()),
                    }
                    code { class: "text-xs text-slate", "{s.key}" }
                    label { class: "inline-flex items-center gap-1 text-xs text-gray-600",
                        input {
                            r#type: "radio",
                            name: "app-home",
                            checked: home == s.key,
                            onchange: {
                                let key = s.key.clone();
                                move |_| draft.with_mut(|d| d.home = Some(key.clone()))
                            },
                        }
                        {t!("apps-home")}
                    }
                    button {
                        r#type: "button",
                        class: BTN_SECONDARY_SM,
                        disabled: i == 0,
                        title: t!("apps-move-up").to_string(),
                        onclick: move |_| draft.with_mut(|d| d.screens.swap(i, i - 1)),
                        "↑"
                    }
                    button {
                        r#type: "button",
                        class: BTN_SECONDARY_SM,
                        disabled: i + 1 == count,
                        title: t!("apps-move-down").to_string(),
                        onclick: move |_| draft.with_mut(|d| d.screens.swap(i, i + 1)),
                        "↓"
                    }
                    if let Some(on_server) = screens_on_server.iter().find(|x| x.key == s.key).cloned() {
                        button {
                            r#type: "button",
                            class: BTN_SECONDARY_SM,
                            "data-testid": "screen-edit",
                            onclick: move |_| {
                                navigator
                                    .push(Route::Dashboards {
                                        id: on_server.dashboard_id.clone(),
                                        mode: "edit".to_string(),
                                    });
                            },
                            {t!("apps-screen-edit")}
                        }
                    } else {
                        span { class: "text-xs text-gray-500", {t!("apps-screen-save-first")} }
                    }
                    button {
                        r#type: "button",
                        class: BTN_SECONDARY_SM,
                        onclick: move |_| {
                            draft
                                .with_mut(|d| {
                                    let gone = d.screens.remove(i);
                                    if d.home.as_ref() == Some(&gone.key) {
                                        d.home = None;
                                    }
                                });
                        },
                        {t!("common-delete")}
                    }
                }
            }
            button {
                r#type: "button",
                class: BTN_SECONDARY_SM,
                "data-testid": "screen-add",
                onclick: move |_| {
                    let name = t!("apps-screen-new").to_string();
                    draft
                        .with_mut(|d| {
                            let key = free_key("screen", d.screens.iter().map(|s| s.key.as_str()));
                            d.screens
                                .push(AppScreenInput {
                                    key,
                                    name,
                                    icon: None,
                                });
                        });
                },
                {t!("apps-screen-add")}
            }
        }
    }
}

const KINDS: [(VariableKind, &str); 5] = [
    (VariableKind::Text, "text"),
    (VariableKind::Number, "number"),
    (VariableKind::Date, "date"),
    (VariableKind::Object, "object"),
    (VariableKind::Enum, "enum"),
];

fn kind_label(kind: VariableKind) -> String {
    match kind {
        VariableKind::Text => t!("apps-var-text").to_string(),
        VariableKind::Number => t!("apps-var-number").to_string(),
        VariableKind::Date => t!("apps-var-date").to_string(),
        VariableKind::Object => t!("apps-var-object").to_string(),
        VariableKind::Enum => t!("apps-var-enum").to_string(),
    }
}

#[component]
fn VariablesPanel(draft: Signal<AppBuildInput>, types: Vec<ObjectTypeView>) -> Element {
    let current = draft.read().variables.clone();
    rsx! {
        section { class: "{PANEL} {SECTION}", "data-testid": "app-variables",
            h2 { class: "text-sm font-semibold text-ink", {t!("apps-variables")} }
            p { class: FIELD_HELP, {t!("apps-variables-help")} }
            for (i, v) in current.into_iter().enumerate() {
                VariableRow {
                    key: "{i}",
                    draft,
                    i,
                    v,
                    types: types.clone(),
                }
            }
            button {
                r#type: "button",
                class: BTN_SECONDARY_SM,
                "data-testid": "variable-add",
                onclick: move |_| {
                    draft
                        .with_mut(|d| {
                            let name = free_key("var", d.variables.iter().map(|v| v.name.as_str()));
                            d.variables
                                .push(AppVariable {
                                    name,
                                    ..Default::default()
                                });
                        });
                },
                {t!("apps-variable-add")}
            }
        }
    }
}

#[component]
fn VariableRow(
    draft: Signal<AppBuildInput>,
    i: usize,
    v: AppVariable,
    types: Vec<ObjectTypeView>,
) -> Element {
    let needs_type = matches!(v.kind, VariableKind::Object | VariableKind::Enum);
    let type_key = v.type_key.clone().unwrap_or_default();
    let property = v.property.clone().unwrap_or_default();
    let enum_props: Vec<String> = types
        .iter()
        .find(|t| t.def.key == type_key)
        .map(|t| {
            t.def
                .properties
                .iter()
                .filter(|p| matches!(p.kind, pnex_core::ontology::PropertyKind::Enum { .. }))
                .map(|p| p.key.clone())
                .collect()
        })
        .unwrap_or_default();
    rsx! {
        div { class: ROW,
            input {
                class: "{INPUT} max-w-[12rem] font-mono",
                value: "{v.name}",
                oninput: move |e| draft.with_mut(|d| d.variables[i].name = e.value()),
            }
            select {
                class: "{INPUT} max-w-[10rem]",
                onchange: move |e| {
                    let picked = KINDS.iter().find(|(_, k)| *k == e.value()).map(|(k, _)| *k);
                    draft
                        .with_mut(|d| {
                            d.variables[i].kind = picked.unwrap_or_default();
                        });
                },
                for (k, value) in KINDS {
                    option { key: "{value}", value, selected: k == v.kind, {kind_label(k)} }
                }
            }
            if needs_type {
                select {
                    class: "{INPUT} max-w-[12rem]",
                    onchange: move |e| {
                        let value = e.value();
                        draft
                            .with_mut(|d| {
                                d.variables[i].type_key = (!value.is_empty()).then_some(value);
                                d.variables[i].property = None;
                            });
                    },
                    option { value: "", selected: type_key.is_empty(), "—" }
                    for t in types.iter() {
                        option {
                            key: "{t.def.key}",
                            value: "{t.def.key}",
                            selected: t.def.key == type_key,
                            {type_label(&t.def)}
                        }
                    }
                }
            }
            if v.kind == VariableKind::Enum {
                select {
                    class: "{INPUT} max-w-[12rem]",
                    onchange: move |e| {
                        let value = e.value();
                        draft
                            .with_mut(|d| {
                                d.variables[i].property = (!value.is_empty()).then_some(value);
                            });
                    },
                    option { value: "", selected: property.is_empty(), "—" }
                    for p in enum_props {
                        option {
                            key: "{p}",
                            value: "{p}",
                            selected: p == property,
                            "{p}"
                        }
                    }
                }
            }
            if v.kind != VariableKind::Object {
                input {
                    class: "{INPUT} max-w-[12rem]",
                    placeholder: t!("apps-variable-default").to_string(),
                    value: "{v.default.clone().unwrap_or_default()}",
                    oninput: move |e| {
                        let value = e.value();
                        draft
                            .with_mut(|d| {
                                d.variables[i].default = (!value.is_empty()).then_some(value);
                            });
                    },
                }
            }
            button {
                r#type: "button",
                class: BTN_SECONDARY_SM,
                onclick: move |_| {
                    draft
                        .with_mut(|d| {
                            d.variables.remove(i);
                        });
                },
                {t!("common-delete")}
            }
        }
    }
}

#[component]
fn SetsPanel(draft: Signal<AppBuildInput>, types: Vec<ObjectTypeView>) -> Element {
    let sets: BTreeMap<String, ObjectSetDef> = draft.read().object_sets.clone();
    let first_type = types.first().map(|t| t.def.key.clone());
    rsx! {
        section { class: "{PANEL} {SECTION}", "data-testid": "app-sets",
            h2 { class: "text-sm font-semibold text-ink", {t!("apps-sets")} }
            p { class: FIELD_HELP, {t!("apps-sets-help")} }
            for (name, set) in sets {
                SetEditor {
                    key: "{name}",
                    draft,
                    name: name.clone(),
                    set,
                    types: types.clone(),
                }
            }
            button {
                r#type: "button",
                class: BTN_SECONDARY_SM,
                "data-testid": "set-add",
                disabled: first_type.is_none(),
                onclick: move |_| {
                    let type_key = first_type.clone();
                    draft
                        .with_mut(|d| {
                            let name = free_key("set", d.object_sets.keys().map(String::as_str));
                            d.object_sets
                                .insert(
                                    name,
                                    ObjectSetDef {
                                        description: String::new(),
                                        query: OntologyQuery {
                                            type_key,
                                            ..Default::default()
                                        },
                                    },
                                );
                        });
                },
                {t!("apps-set-add")}
            }
        }
    }
}

#[component]
fn SetEditor(
    draft: Signal<AppBuildInput>,
    name: String,
    set: ObjectSetDef,
    types: Vec<ObjectTypeView>,
) -> Element {
    let type_key = set.query.type_key.clone().unwrap_or_default();
    let props: Vec<(String, String)> = types
        .iter()
        .find(|t| t.def.key == type_key)
        .map(|t| {
            t.def
                .properties
                .iter()
                .map(|p| {
                    (
                        p.key.clone(),
                        crate::components::ontology::property_input::prop_label(p),
                    )
                })
                .collect()
        })
        .unwrap_or_default();
    let order_by = set.query.order_by.clone().unwrap_or_default();
    let descending = set.query.descending;
    let filters = set.query.filters.clone();
    // Every handler edits this set's query by name: no index into a moving map.
    let edit = {
        let name = name.clone();
        move |f: &mut dyn FnMut(&mut OntologyQuery)| {
            draft.with_mut(|d| {
                if let Some(s) = d.object_sets.get_mut(&name) {
                    f(&mut s.query);
                }
            });
        }
    };
    let on_type = edit.clone();
    let on_order = edit.clone();
    let on_desc = edit.clone();
    let on_add = edit.clone();
    let remove_name = name.clone();
    let rename_from = name.clone();
    rsx! {
        div {
            class: "space-y-2 rounded-lg border border-gray-200 p-3",
            "data-set": "{name}",
            div { class: ROW,
                input {
                    class: "{INPUT} max-w-[12rem] font-mono",
                    value: "{name}",
                    // On leaving the field: a rename per key stroke would remount it.
                    onchange: move |e| {
                        let to = e.value();
                        let from = rename_from.clone();
                        draft
                            .with_mut(|d| {
                                if d.object_sets.contains_key(&to) {
                                    return;
                                }
                                if let Some(s) = d.object_sets.remove(&from) {
                                    d.object_sets.insert(to, s);
                                }
                            });
                    },
                }
                select {
                    class: "{INPUT} max-w-[14rem]",
                    onchange: move |e| {
                        let value = e.value();
                        let mut edit = on_type.clone();
                        edit(
                            &mut |q| {
                                q.type_key = Some(value.clone());
                                q.filters.clear();
                                q.order_by = None;
                            },
                        );
                    },
                    for t in types.iter() {
                        option {
                            key: "{t.def.key}",
                            value: "{t.def.key}",
                            selected: t.def.key == type_key,
                            {type_label(&t.def)}
                        }
                    }
                }
                button {
                    r#type: "button",
                    class: BTN_SECONDARY_SM,
                    onclick: move |_| {
                        let name = remove_name.clone();
                        draft
                            .with_mut(|d| {
                                d.object_sets.remove(&name);
                            });
                    },
                    {t!("common-delete")}
                }
            }
            for (i, f) in filters.into_iter().enumerate() {
                FilterRow {
                    key: "{i}",
                    i,
                    filter: f,
                    props: props.clone(),
                    placeholder: "$parameter",
                    on_change: {
                        let set = name.clone();
                        move |next| {
                            draft
                                .with_mut(|d| {
                                    if let Some(x) = d
                                        .object_sets
                                        .get_mut(&set)
                                        .and_then(|s| s.query.filters.get_mut(i))
                                    {
                                        *x = next;
                                    }
                                });
                        }
                    },
                    on_remove: {
                        let set = name.clone();
                        move |_| {
                            draft
                                .with_mut(|d| {
                                    if let Some(s) = d.object_sets.get_mut(&set) {
                                        s.query.filters.remove(i);
                                    }
                                });
                        }
                    },
                }
            }
            button {
                r#type: "button",
                class: BTN_SECONDARY_SM,
                disabled: props.is_empty(),
                "data-testid": "filter-add",
                onclick: {
                    let first = props.first().map(|p| p.0.clone()).unwrap_or_default();
                    move |_| {
                        let key = first.clone();
                        let mut edit = on_add.clone();
                        edit(
                            &mut |q| {
                                q.filters
                                    .push(PropertyFilter {
                                        key: key.clone(),
                                        op: FilterOp::Eq,
                                        value: "".into(),
                                    });
                            },
                        );
                    }
                },
                {t!("apps-filter-add")}
            }
            div { class: ROW,
                label { class: FIELD_LABEL, {t!("apps-sort")} }
                select {
                    class: "{INPUT} max-w-[14rem]",
                    onchange: move |e| {
                        let value = e.value();
                        let mut edit = on_order.clone();
                        edit(
                            &mut |q| {
                                q.order_by = (!value.is_empty()).then(|| value.clone());
                            },
                        );
                    },
                    option { value: "", selected: order_by.is_empty(), {t!("apps-col-title")} }
                    for (k, l) in props.iter().cloned() {
                        option {
                            key: "{k}",
                            value: "{k}",
                            selected: k == order_by,
                            "{l}"
                        }
                    }
                }
                label { class: "inline-flex items-center gap-1 text-xs text-gray-600",
                    input {
                        r#type: "checkbox",
                        checked: descending,
                        onchange: move |e| {
                            let on = e.checked();
                            let mut edit = on_desc.clone();
                            edit(&mut |q| q.descending = on);
                        },
                    }
                    {t!("apps-sort-descending")}
                }
            }
        }
    }
}
