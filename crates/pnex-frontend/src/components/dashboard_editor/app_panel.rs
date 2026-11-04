//! Inspector section of a widget of an application screen (app-builder.md
//! D508–D511): what it reads and writes, from what its application has.

use std::collections::BTreeMap;

use dioxus::prelude::*;
use dioxus_i18n::t;
use pnex_core::apps::{params_of, AppWidgetOptions, VariableKind};
use pnex_core::Widget;

use super::EditorCx;
use crate::api;
use crate::components::ontology::property_input::prop_label;
use crate::components::ui::{FIELD_LABEL, INPUT};

#[component]
pub fn AppWidgetPanel(cx: EditorCx, widget: Widget, can_write: bool) -> Element {
    let mut layout = cx.layout;
    let types = use_resource(|| async { api::ontology::types().await.unwrap_or_default() });
    let Some(app) = cx.app.read().clone() else {
        return rsx! {};
    };
    let o = widget.options.app.clone().unwrap_or_default();
    let kind = widget.widget_type.clone();
    let id = widget.id.clone();
    // One entry point for every field of the panel.
    let edit = move |f: &mut dyn FnMut(&mut AppWidgetOptions)| {
        let id = id.clone();
        layout.with_mut(|l| {
            if let Some(w) = l.widgets.iter_mut().find(|w| w.id == id) {
                f(w.options.app.get_or_insert_with(Default::default));
            }
        });
    };
    let reads_set = kind == "object_table";
    let set = o.object_set.as_deref().and_then(|n| app.object_sets.get(n));
    let params: Vec<String> = set.map(|s| params_of(&s.query)).unwrap_or_default();
    let vars = app.variables.clone();
    let object_vars: Vec<String> = vars
        .iter()
        .filter(|v| v.kind == VariableKind::Object)
        .map(|v| v.name.clone())
        .collect();
    let value_vars: Vec<String> = vars
        .iter()
        .filter(|v| v.kind != VariableKind::Object)
        .map(|v| v.name.clone())
        .collect();
    let all_vars: Vec<String> = vars.iter().map(|v| v.name.clone()).collect();
    // The type whose properties a table or a record shows.
    let shown_type = match kind.as_str() {
        "object_fields" => o
            .variable
            .as_deref()
            .and_then(|n| vars.iter().find(|v| v.name == n))
            .and_then(|v| v.type_key.clone()),
        _ => set.and_then(|s| s.query.type_key.clone()),
    };
    let all_types = types.read().clone().unwrap_or_default();
    let columns: Vec<(String, String)> = all_types
        .iter()
        .find(|t| Some(&t.def.key) == shown_type.as_ref())
        .map(|t| {
            t.def
                .properties
                .iter()
                .map(|p| (p.key.clone(), prop_label(p)))
                .collect()
        })
        .unwrap_or_default();
    let scope_types: Vec<(String, String)> = all_types
        .iter()
        .filter(|t| app.scope.types.contains(&t.def.key))
        .map(|t| {
            (
                t.def.key.clone(),
                crate::components::ontology::type_label(&t.def),
            )
        })
        .collect();
    let screens: Vec<(String, String)> = app
        .screens
        .iter()
        .map(|s| (s.key.clone(), s.name.clone()))
        .collect();
    let on_set = edit.clone();
    let on_variable = edit.clone();
    let on_screen = edit.clone();
    let on_type = edit.clone();
    let on_action = edit.clone();
    let on_params = edit.clone();
    let on_column = edit;
    let shows_columns = matches!(kind.as_str(), "object_table" | "object_fields");
    let (variable_label, variable_choices, shows_variable) = match kind.as_str() {
        "object_table" => (t!("insp-app-row-variable"), object_vars.clone(), true),
        "object_fields" | "action_button" => (t!("insp-app-variable"), object_vars.clone(), true),
        "object_form" => (t!("insp-app-form-variable"), object_vars.clone(), true),
        "input" => (t!("insp-app-variable"), value_vars, true),
        _ => (t!("insp-app-variable"), Vec::new(), false),
    };
    rsx! {
        div {
            class: "space-y-2 border-t border-gray-100 pt-3",
            "data-testid": "app-widget-panel",
            if reads_set {
                label { class: FIELD_LABEL, {t!("insp-app-set")} }
                select {
                    class: INPUT,
                    disabled: !can_write,
                    "data-testid": "insp-app-set",
                    onchange: move |e| {
                        let v = e.value();
                        let mut edit = on_set.clone();
                        edit(
                            &mut |o| {
                                o.object_set = (!v.is_empty()).then(|| v.clone());
                                o.params = BTreeMap::new();
                                o.columns.clear();
                            },
                        );
                    },
                    option { value: "", selected: o.object_set.is_none(), {t!("insp-app-none")} }
                    for k in app.object_sets.keys().cloned() {
                        option {
                            key: "{k}",
                            value: "{k}",
                            selected: o.object_set.as_deref() == Some(k.as_str()),
                            "{k}"
                        }
                    }
                }
                for p in params {
                    ParamSelect {
                        key: "{p}",
                        param: p.clone(),
                        current: o.params.get(&p).cloned().unwrap_or_default(),
                        variables: all_vars.clone(),
                        can_write,
                        on_pick: {
                            let p = p.clone();
                            let edit = on_params.clone();
                            move |v: String| {
                                let mut edit = edit.clone();
                                edit(
                                    &mut |o| {
                                        o.params.insert(p.clone(), v.clone());
                                    },
                                );
                            }
                        },
                    }
                }
            }
            if kind == "object_form" {
                label { class: FIELD_LABEL, {t!("insp-app-type")} }
                select {
                    class: INPUT,
                    disabled: !can_write,
                    "data-testid": "insp-app-type",
                    onchange: move |e| {
                        let v = e.value();
                        let mut edit = on_type.clone();
                        edit(&mut |o| o.object_type = (!v.is_empty()).then(|| v.clone()));
                    },
                    option { value: "", selected: o.object_type.is_none(), {t!("insp-app-none")} }
                    for (k, l) in scope_types {
                        option {
                            key: "{k}",
                            value: "{k}",
                            selected: o.object_type.as_deref() == Some(k.as_str()),
                            "{l}"
                        }
                    }
                }
            }
            if kind == "action_button" {
                label { class: FIELD_LABEL, {t!("insp-app-action")} }
                select {
                    class: INPUT,
                    disabled: !can_write,
                    "data-testid": "insp-app-action",
                    onchange: move |e| {
                        let v = e.value();
                        let mut edit = on_action.clone();
                        edit(&mut |o| o.action = (!v.is_empty()).then(|| v.clone()));
                    },
                    option { value: "", selected: o.action.is_none(), {t!("insp-app-none")} }
                    for a in app.actions.iter().cloned() {
                        option {
                            key: "{a}",
                            value: "{a}",
                            selected: o.action.as_deref() == Some(a.as_str()),
                            "{a}"
                        }
                    }
                }
            }
            if shows_variable {
                label { class: FIELD_LABEL, "{variable_label}" }
                select {
                    class: INPUT,
                    disabled: !can_write,
                    "data-testid": "insp-app-variable",
                    onchange: move |e| {
                        let v = e.value();
                        let mut edit = on_variable.clone();
                        edit(&mut |o| o.variable = (!v.is_empty()).then(|| v.clone()));
                    },
                    option { value: "", selected: o.variable.is_none(), {t!("insp-app-none")} }
                    for v in variable_choices {
                        option {
                            key: "{v}",
                            value: "{v}",
                            selected: o.variable.as_deref() == Some(v.as_str()),
                            "{v}"
                        }
                    }
                }
            }
            if kind == "object_table" {
                label { class: FIELD_LABEL, {t!("insp-app-screen")} }
                select {
                    class: INPUT,
                    disabled: !can_write,
                    "data-testid": "insp-app-screen",
                    onchange: move |e| {
                        let v = e.value();
                        let mut edit = on_screen.clone();
                        edit(&mut |o| o.screen = (!v.is_empty()).then(|| v.clone()));
                    },
                    option { value: "", selected: o.screen.is_none(), {t!("insp-app-none")} }
                    for (k, l) in screens {
                        option {
                            key: "{k}",
                            value: "{k}",
                            selected: o.screen.as_deref() == Some(k.as_str()),
                            "{l}"
                        }
                    }
                }
            }
            if shows_columns {
                label { class: FIELD_LABEL, {t!("insp-app-columns")} }
                for (k, l) in columns {
                    ColumnCheck {
                        key: "{k}",
                        label: l,
                        checked: o.columns.contains(&k),
                        can_write,
                        on_toggle: {
                            let edit = on_column.clone();
                            let k = k.clone();
                            move |on: bool| {
                                let mut edit = edit.clone();
                                edit(
                                    &mut |o| {
                                        o.columns.retain(|c| c != &k);
                                        if on {
                                            o.columns.push(k.clone());
                                        }
                                    },
                                );
                            }
                        },
                    }
                }
            }
        }
    }
}

#[component]
fn ParamSelect(
    param: String,
    current: String,
    variables: Vec<String>,
    can_write: bool,
    on_pick: EventHandler<String>,
) -> Element {
    rsx! {
        label { class: FIELD_LABEL, {t!("insp-app-param", name : param.clone())} }
        select {
            class: INPUT,
            disabled: !can_write,
            "data-app-param": "{param}",
            onchange: move |e| on_pick.call(e.value()),
            option { value: "", selected: current.is_empty(), {t!("insp-app-none")} }
            for v in variables {
                option { key: "{v}", value: "{v}", selected: current == v, "{v}" }
            }
        }
    }
}

#[component]
fn ColumnCheck(
    label: String,
    checked: bool,
    can_write: bool,
    on_toggle: EventHandler<bool>,
) -> Element {
    rsx! {
        label { class: "flex items-center gap-2 text-xs text-gray-700",
            input {
                r#type: "checkbox",
                checked,
                disabled: !can_write,
                onchange: move |e| on_toggle.call(e.checked()),
            }
            "{label}"
        }
    }
}
