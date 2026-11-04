//! Widgets of an application screen (app-builder.md D510, D511): they read
//! the application's object sets with the reader's rights and share values
//! through its variables. Values are shown as text, never as HTML (R11–R13).
//! Outside a running application (the editor's canvas) they show what they
//! are, with no data.

use std::collections::BTreeMap;

use dioxus::prelude::*;
use dioxus_i18n::t;
use pnex_core::apps::{AppVariable, AppView, AppWidgetOptions, VariableKind};
use pnex_core::ontology::api::{ObjectTypeView, ObjectView, QueryResult};
use pnex_core::ontology::{PropertyDef, PropertyKind};
use pnex_core::Widget;

use crate::api;
use crate::app::Route;
use crate::components::ontology::property_input::{prop_label, PropertyValue};
use crate::components::ui::{BTN_SECONDARY_SM, INPUT};
use crate::state::toasts;

/// Rows per page of a table.
const PAGE: u64 = 20;

/// The running application, given by its view to its widgets.
#[derive(Clone, Copy, PartialEq)]
pub struct AppRunCx {
    pub app: Signal<AppView>,
    pub types: Signal<Vec<ObjectTypeView>>,
    /// Values of the variables: local to this session, never stored (D509).
    pub vars: Signal<BTreeMap<String, String>>,
    /// Bumped by the refresh cadence and after a write.
    pub tick: Signal<u32>,
}

impl AppRunCx {
    fn variable(&self, name: &str) -> Option<AppVariable> {
        self.app
            .read()
            .variables
            .iter()
            .find(|v| v.name == name)
            .cloned()
    }

    fn value(&self, name: Option<&str>) -> Option<String> {
        self.vars
            .read()
            .get(name?)
            .filter(|v| !v.is_empty())
            .cloned()
    }

    fn property(&self, type_key: &str, key: &str) -> Option<PropertyDef> {
        self.types
            .read()
            .iter()
            .find(|t| t.def.key == type_key)?
            .def
            .properties
            .iter()
            .find(|p| p.key == key)
            .cloned()
    }
}

/// Parameters of the set filled from the variables; `None` while one is
/// empty: the widget asks for it and sends nothing (D508).
fn params_of(cx: &AppRunCx, o: &AppWidgetOptions) -> Option<BTreeMap<String, String>> {
    o.params
        .iter()
        .map(|(p, v)| Some((p.clone(), cx.value(Some(v))?)))
        .collect()
}

#[component]
pub fn AppWidgetBody(widget: Widget) -> Element {
    let o = widget.options.app.clone().unwrap_or_default();
    // The title typed in the editor wins over what a pack shipped.
    let title = if widget.title.trim().is_empty() {
        crate::components::ontology::tr("", &o.titles)
    } else {
        widget.title.clone()
    };
    let Some(cx) = try_use_context::<AppRunCx>() else {
        return rsx! {
            div { class: "flex h-full w-full flex-col overflow-hidden rounded-lg border border-dashed border-gray-300 p-3 text-xs text-gray-500",
                if !title.trim().is_empty() {
                    span { class: "font-medium", "{title}" }
                }
                span { {crate::components::dashboard_editor::library::kind_label(&widget.widget_type)} }
            }
        };
    };
    rsx! {
        div {
            class: "flex h-full w-full flex-col overflow-hidden rounded-lg bg-surface",
            "data-app-widget": "{widget.widget_type}",
            if !title.trim().is_empty() {
                div { class: "px-3 pt-2 truncate text-xs font-medium text-gray-500",
                    "{title}"
                }
            }
            match widget.widget_type.as_str() {
                "object_table" => rsx! {
                    ObjectTable { cx, o }
                },
                "object_fields" => rsx! {
                    ObjectFields { cx, o }
                },
                "input" => rsx! {
                    VariableInput { cx, o }
                },
                "object_form" => rsx! {
                    ObjectFormButton { cx, o }
                },
                "action_button" => rsx! {
                    ActionButton { cx, o }
                },
                _ => rsx! {},
            }
        }
    }
}

#[component]
fn Pending(text: String) -> Element {
    rsx! {
        p {
            class: "p-3 text-xs text-gray-500",
            "data-testid": "app-widget-pending",
            "{text}"
        }
    }
}

/// A set's result once its parameters are filled; `None` = still waiting.
fn use_set(
    cx: AppRunCx,
    o: AppWidgetOptions,
    limit: u64,
    offset: Signal<u64>,
    sort: Signal<Option<(String, bool)>>,
) -> Resource<Option<Result<QueryResult, api::error::ApiError>>> {
    use_resource(move || {
        let _ = (cx.tick)();
        let params = params_of(&cx, &o);
        let app_id = cx.app.read().id.clone();
        let name = o.object_set.clone().unwrap_or_default();
        let from = offset();
        let by = sort();
        async move {
            let params = params?;
            Some(api::apps::run_set(&app_id, &name, &params, limit, from, by).await)
        }
    })
}

#[component]
fn ObjectTable(cx: AppRunCx, o: AppWidgetOptions) -> Element {
    let mut offset = use_signal(|| 0u64);
    let mut sort = use_signal(|| None::<(String, bool)>);
    let data = use_set(cx, o.clone(), PAGE, offset, sort);
    let state = data.read().clone();
    let columns = if o.columns.is_empty() {
        vec!["title".to_string()]
    } else {
        o.columns.clone()
    };
    let result = match state {
        None => {
            return rsx! {
                Pending { text: t!("common-loading").to_string() }
            }
        }
        Some(None) => {
            return rsx! {
                Pending { text: t!("apps-filter-pending").to_string() }
            }
        }
        Some(Some(Err(e))) => {
            return rsx! {
                Pending { text: crate::api::error_i18n::localize(&e) }
            }
        }
        Some(Some(Ok(r))) => r,
    };
    let count = result.count.max(0) as u64;
    let from = offset();
    let type_key = o
        .object_set
        .as_deref()
        .and_then(|n| cx.app.read().object_sets.get(n)?.query.type_key.clone())
        .unwrap_or_default();
    let header: Vec<(String, String)> = columns
        .iter()
        .map(|c| {
            let label = if c == "title" {
                t!("apps-col-title").to_string()
            } else {
                cx.property(&type_key, c)
                    .map(|p| prop_label(&p))
                    .unwrap_or_else(|| c.clone())
            };
            (c.clone(), label)
        })
        .collect();
    let (variable, screen) = (o.variable.clone(), o.screen.clone());
    rsx! {
        div { class: "flex-1 overflow-auto",
            table { class: "w-full text-left text-sm",
                thead { class: "text-xs text-gray-500",
                    tr {
                        for (key, label) in header {
                            th { key: "{key}", class: "px-3 py-2 font-medium",
                                button {
                                    r#type: "button",
                                    class: "hover:underline",
                                    disabled: key == "title",
                                    onclick: move |_| {
                                        let descending = sort.peek().as_ref().is_some_and(|(k, d)| *k == key && !*d);
                                        sort.set(Some((key.clone(), descending)));
                                        offset.set(0);
                                    },
                                    "{label}"
                                }
                            }
                        }
                    }
                }
                tbody {
                    for row in result.rows {
                        TableRow {
                            key: "{row.object.id}",
                            cx,
                            object: row.object.clone(),
                            columns: columns.clone(),
                            variable: variable.clone(),
                            screen: screen.clone(),
                        }
                    }
                }
            }
        }
        if count > PAGE {
            div { class: "flex items-center justify-between border-t border-gray-100 px-3 py-1 text-xs text-gray-500",
                button {
                    r#type: "button",
                    class: BTN_SECONDARY_SM,
                    disabled: from == 0,
                    onclick: move |_| offset.set(from.saturating_sub(PAGE)),
                    {t!("apps-page-previous")}
                }
                span { {t!("apps-page-range", from : (from + 1) as i64, count : count as i64)} }
                button {
                    r#type: "button",
                    class: BTN_SECONDARY_SM,
                    disabled: from + PAGE >= count,
                    onclick: move |_| offset.set(from + PAGE),
                    {t!("apps-page-next")}
                }
            }
        }
    }
}

#[component]
fn TableRow(
    cx: AppRunCx,
    object: ObjectView,
    columns: Vec<String>,
    variable: Option<String>,
    screen: Option<String>,
) -> Element {
    let mut vars = cx.vars;
    let navigator = use_navigator();
    let id = object.id.clone();
    let selected = variable
        .as_deref()
        .is_some_and(|v| cx.value(Some(v)).as_deref() == Some(id.as_str()));
    let clickable = variable.is_some() || screen.is_some();
    let class = match (selected, clickable) {
        (true, _) => "border-t border-gray-100 bg-signal-50 cursor-pointer",
        (false, true) => "border-t border-gray-100 hover:bg-gray-50 cursor-pointer",
        (false, false) => "border-t border-gray-100",
    };
    rsx! {
        tr {
            class,
            onclick: move |_| {
                if let Some(v) = &variable {
                    vars.with_mut(|m| {
                        m.insert(v.clone(), id.clone());
                    });
                }
                if let Some(s) = screen.clone().filter(|s| !s.is_empty()) {
                    let key = cx.app.read().key.clone();
                    navigator
                        .push(Route::AppRun {
                            app: key,
                            screen: s,
                            query: String::new(),
                        });
                }
            },
            for c in columns {
                td { key: "{c}", class: "px-3 py-2",
                    CellValue { cx, object: object.clone(), column: c.clone() }
                }
            }
        }
    }
}

#[component]
fn CellValue(cx: AppRunCx, object: ObjectView, column: String) -> Element {
    if column == "title" {
        return rsx! { "{object.title}" };
    }
    if object.redacted.contains(&column) {
        return rsx! {
            span { class: "text-gray-500", {t!("onto-masked")} }
        };
    }
    let Some(def) = cx.property(&object.type_key, &column) else {
        return rsx! {};
    };
    rsx! {
        PropertyValue { value: object.properties.get(&column).cloned(), def }
    }
}

/// The object a variable names, reloaded with it and with the cadence.
fn use_object(
    cx: AppRunCx,
    variable: Option<String>,
) -> Resource<Option<Result<ObjectView, api::error::ApiError>>> {
    use_resource(move || {
        let _ = (cx.tick)();
        let id = cx.value(variable.as_deref());
        async move {
            let id = id?;
            Some(api::ontology::object(&id).await)
        }
    })
}

#[component]
fn ObjectFields(cx: AppRunCx, o: AppWidgetOptions) -> Element {
    let data = use_object(cx, o.variable.clone());
    let state = data.read().clone();
    let object = match state {
        Some(Some(Ok(object))) => object,
        Some(Some(Err(e))) => {
            return rsx! {
                Pending { text: crate::api::error_i18n::localize(&e) }
            }
        }
        _ => {
            return rsx! {
                Pending { text: t!("apps-object-pending").to_string() }
            }
        }
    };
    // A screen shows nothing out of its application's scope, even an object
    // the reader could open elsewhere.
    if !cx.app.read().scope.types.contains(&object.type_key) {
        return rsx! {
            Pending { text: t!("err-app-out-of-scope").to_string() }
        };
    }
    let columns: Vec<PropertyDef> = o
        .columns
        .iter()
        .filter_map(|c| cx.property(&object.type_key, c))
        .collect();
    rsx! {
        div { class: "flex-1 overflow-auto p-3 space-y-2 text-sm",
            Link {
                class: "font-medium text-signal-700 hover:underline",
                to: Route::OntologyObject {
                    id: object.id.clone(),
                },
                "{object.title}"
            }
            dl { class: "grid grid-cols-1 gap-2 sm:grid-cols-2",
                for p in columns {
                    div { key: "{p.key}",
                        dt { class: "text-xs text-gray-500", {prop_label(&p)} }
                        dd {
                            CellValue {
                                cx,
                                object: object.clone(),
                                column: p.key.clone(),
                            }
                        }
                    }
                }
            }
        }
    }
}

#[component]
fn VariableInput(cx: AppRunCx, o: AppWidgetOptions) -> Element {
    let mut vars = cx.vars;
    let Some(variable) = o.variable.as_deref().and_then(|v| cx.variable(v)) else {
        return rsx! {};
    };
    let name = variable.name.clone();
    let current = vars.read().get(&name).cloned().unwrap_or_default();
    let choices: Vec<(String, String)> = match (&variable.type_key, &variable.property) {
        (Some(t), Some(p)) if variable.kind == VariableKind::Enum => cx
            .property(t, p)
            .map(|def| match &def.kind {
                PropertyKind::Enum { values } => values
                    .iter()
                    .map(|v| (v.clone(), crate::components::ontology::value_label(&def, v)))
                    .collect(),
                _ => Vec::new(),
            })
            .unwrap_or_default(),
        _ => Vec::new(),
    };
    let input_type = match variable.kind {
        VariableKind::Number => "number",
        VariableKind::Date => "date",
        _ => "text",
    };
    let write = move |value: String| {
        let name = name.clone();
        vars.with_mut(|m| {
            m.insert(name, value);
        });
    };
    let mut on_select = write.clone();
    let mut on_input = write;
    rsx! {
        div { class: "p-3",
            if variable.kind == VariableKind::Enum {
                select {
                    class: INPUT,
                    "data-app-input": "{variable.name}",
                    onchange: move |e| on_select(e.value()),
                    option { value: "", selected: current.is_empty(), "—" }
                    for (k, l) in choices {
                        option {
                            key: "{k}",
                            value: "{k}",
                            selected: current == k,
                            "{l}"
                        }
                    }
                }
            } else {
                input {
                    class: INPUT,
                    r#type: input_type,
                    "data-app-input": "{variable.name}",
                    value: "{current}",
                    onchange: move |e| on_input(e.value()),
                }
            }
        }
    }
}

/// Creates an object of its type, or edits the one its variable names,
/// through the object form of the ontology: same validation, same 409 (D511).
#[component]
fn ObjectFormButton(cx: AppRunCx, o: AppWidgetOptions) -> Element {
    let mut tick = cx.tick;
    let mut open = use_signal(|| false);
    let data = use_object(cx, o.variable.clone());
    let can_write = crate::state::access::can("ontology:write");
    let initial = match data.read().clone() {
        Some(Some(Ok(object))) if Some(&object.type_key) == o.object_type.as_ref() => Some(object),
        _ => None,
    };
    let type_view = cx
        .types
        .read()
        .iter()
        .find(|t| Some(&t.def.key) == o.object_type.as_ref())
        .cloned();
    let Some(type_view) = type_view else {
        return rsx! {};
    };
    // With a variable it edits what the variable names, nothing while empty.
    let edits = o.variable.as_deref().is_some_and(|v| !v.is_empty());
    let nothing_to_edit = edits && initial.is_none();
    let label = if edits {
        t!("apps-form-edit").to_string()
    } else {
        t!("apps-form-create").to_string()
    };
    let saved = t!("apps-form-saved").to_string();
    let app_id = cx.app.read().id.clone();
    rsx! {
        div { class: "p-3",
            button {
                r#type: "button",
                class: BTN_SECONDARY_SM,
                disabled: !can_write || nothing_to_edit,
                "data-testid": "app-form-open",
                onclick: move |_| open.set(true),
                "{label}"
            }
        }
        if open() {
            crate::pages::ontology::ObjectForm {
                type_view,
                initial,
                app_id: Some(app_id),
                on_close: move |_| open.set(false),
                on_saved: move |_| {
                    open.set(false);
                    toasts::success(saved.clone());
                    tick.with_mut(|t| *t += 1);
                },
            }
        }
    }
}

/// Invokes a permitted action on the object its variable names (D511): the
/// dialog of D183, an explicit click; a flow acts, never this widget.
#[component]
fn ActionButton(cx: AppRunCx, o: AppWidgetOptions) -> Element {
    let mut tick = cx.tick;
    let mut open = use_signal(|| false);
    let target = cx.value(o.variable.as_deref());
    let can_write = crate::state::access::can("ontology:write");
    let Some((type_key, key)) = o.action.as_deref().and_then(|a| a.split_once('.')) else {
        return rsx! {};
    };
    let label = cx
        .types
        .read()
        .iter()
        .find(|t| t.def.key == type_key)
        .and_then(|t| t.def.action(key))
        .map(|a| crate::components::ontology::tr(&a.name, &a.names))
        .unwrap_or_else(|| key.to_string());
    let key = key.to_string();
    let app_id = cx.app.read().id.clone();
    rsx! {
        div { class: "p-3",
            button {
                r#type: "button",
                class: BTN_SECONDARY_SM,
                disabled: !can_write || target.is_none(),
                "data-testid": "app-action",
                onclick: move |_| open.set(true),
                "{label}"
            }
        }
        if let (true, Some(id)) = (open(), target.clone()) {
            crate::components::ontology::action_dialog::ActionDialog {
                object_id: id,
                action_key: key,
                app_id: Some(app_id),
                on_close: move |_| open.set(false),
                on_done: move |_| {
                    open.set(false);
                    tick.with_mut(|t| *t += 1);
                },
            }
        }
    }
}
