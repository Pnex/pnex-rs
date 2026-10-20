//! Visual editing of a rule: actions dragged (or clicked) into a column of
//! step blocks, blocks dragged to reorder, the selected step edited in the
//! shell's inspector. The rule stays the structured `Recipe`; YAML is only
//! written from it (`to_yaml`).

use dioxus::prelude::*;
use dioxus_i18n::t;
use pnex_core::site_recipe::{new_step, Recipe, RunStep, Step, StepStatus, ACTIONS};
use serde_json::{json, Map, Value};

use super::action_label;
use crate::components::ui::{BTN_DANGER, FIELD_HELP, INPUT};

/// What is being dragged.
#[derive(Clone, Copy, PartialEq, Debug)]
pub(super) enum Drag {
    Move(usize),
    New(&'static str),
}

/// Applies a drop before `at` (`steps.len()` = at the end).
pub(super) fn drop_at(recipe: &mut Recipe, drag: Drag, at: usize) -> usize {
    match drag {
        Drag::New(action) => {
            let at = at.min(recipe.steps.len());
            if let Some(step) = new_step(action) {
                recipe.steps.insert(at, step);
            }
            at
        }
        Drag::Move(from) => {
            if from >= recipe.steps.len() {
                return at;
            }
            let step = recipe.steps.remove(from);
            let at = if from < at { at - 1 } else { at }.min(recipe.steps.len());
            recipe.steps.insert(at, step);
            at
        }
    }
}

/// Actions to drag into the column (a click appends after the selection).
#[component]
pub(super) fn ActionStrip(
    can_write: bool,
    drag: Signal<Option<Drag>>,
    on_add: EventHandler<&'static str>,
) -> Element {
    let mut drag = drag;
    rsx! {
        div { class: "flex flex-wrap gap-1.5",
            for action in ACTIONS {
                button {
                    key: "{action}",
                    class: "cursor-grab rounded-md border border-line bg-surface px-2 py-1 text-xs font-medium text-ink hover:border-signal-300 hover:bg-signal-50 disabled:opacity-40",
                    draggable: can_write,
                    disabled: !can_write,
                    title: t!("web-sites-drag-hint"),
                    ondragstart: move |_| drag.set(Some(Drag::New(action))),
                    ondragend: move |_| drag.set(None),
                    onclick: move |_| on_add.call(action),
                    "+ {action_label(action)}"
                }
            }
        }
    }
}

/// The column of step blocks.
#[component]
pub(super) fn StepBlocks(
    recipe: Signal<Recipe>,
    selected: Option<usize>,
    on_select: EventHandler<usize>,
    played: Vec<RunStep>,
    can_write: bool,
    drag: Signal<Option<Drag>>,
    on_change: EventHandler<()>,
    on_delete: EventHandler<usize>,
) -> Element {
    let mut recipe = recipe;
    let mut drag = drag;
    let mut over = use_signal(|| None::<usize>);
    let steps = recipe.read().steps.clone();
    let n = steps.len();
    let mut drop_here = move |at: usize| {
        if let Some(d) = drag() {
            let at = recipe.with_mut(|r| drop_at(r, d, at));
            on_select.call(at);
            on_change.call(());
        }
        drag.set(None);
        over.set(None);
    };
    rsx! {
        ol { class: "space-y-1.5",
            for (i, step) in steps.iter().enumerate() {
                {
                    let tone = match played.get(i).map(|s| s.status) {
                        Some(StepStatus::Ok) => "border-green-300 bg-green-50",
                        Some(StepStatus::Failed) => "border-red-400 bg-red-50",
                        Some(StepStatus::Skipped) => "border-line bg-gray-50 opacity-70",
                        None => "border-line bg-surface",
                    };
                    let ring = if selected == Some(i) { "ring-2 ring-signal-400" } else { "" };
                    let gap = if over() == Some(i) { "border-t-4 border-t-signal-400" } else { "" };
                    let label = step.label();
                    let action = step.action();
                    let optional = step.optional();
                    rsx! {
                        li {
                            key: "{i}",
                            class: "flex cursor-pointer items-center gap-2 rounded-md border px-2 py-1.5 text-sm {tone} {ring} {gap}",
                            draggable: can_write,
                            ondragstart: move |_| drag.set(Some(Drag::Move(i))),
                            ondragend: move |_| {
                                drag.set(None);
                                over.set(None);
                            },
                            ondragover: move |e| {
                                e.prevent_default();
                                over.set(Some(i));
                            },
                            ondrop: move |e| {
                                e.prevent_default();
                                drop_here(i);
                            },
                            onclick: move |_| on_select.call(i),
                            span { class: "cursor-grab select-none text-slate", "⋮⋮" }
                            span { class: "w-6 text-right text-slate", "{i + 1}." }
                            span { class: "font-medium text-ink", "{action_label(action)}" }
                            span { class: "truncate text-slate", "{label}" }
                            if optional {
                                span { class: "ml-auto text-xs text-slate", {t!("web-sites-optional")} }
                            }
                        }
                        if selected == Some(i) {
                            li {
                                key: "edit-{i}",
                                class: "rounded-md border border-signal-200 bg-signal-50/40",
                                StepInspector {
                                    recipe,
                                    index: i,
                                    can_write,
                                    on_change: move |_| on_change.call(()),
                                    on_delete: move |_| on_delete.call(i),
                                }
                            }
                        }
                    }
                }
            }
            li {
                class: if over() == Some(n) { "rounded-md border-2 border-dashed border-signal-400 px-2 py-3 text-center text-xs text-signal-700" } else { "rounded-md border-2 border-dashed border-line px-2 py-3 text-center text-xs text-slate" },
                ondragover: move |e| {
                    e.prevent_default();
                    over.set(Some(n));
                },
                ondrop: move |e| {
                    e.prevent_default();
                    drop_here(n);
                },
                {t!("web-sites-drop-here")}
            }
        }
    }
}

// ───────────────────────────── inspector ─────────────────────────────

/// Alternatives of an action (exactly one is set), with their empty value.
fn alternatives(action: &str) -> &'static [(&'static str, fn() -> Value)] {
    fn s() -> Value {
        json!("")
    }
    fn yes() -> Value {
        json!(true)
    }
    fn pair() -> Value {
        json!(["", ""])
    }
    fn ms() -> Value {
        json!(1000)
    }
    fn count() -> Value {
        json!({ "selector": "", "min": 1 })
    }
    match action {
        "click" => &[
            ("text", s),
            ("text_contains", s),
            ("selector", s),
            ("role", s),
        ],
        "wait" => &[("ms", ms), ("text", s)],
        "follow_pdf" => &[("link_text_contains", s), ("href_contains", s)],
        "extract" => &[("between", pair), ("selector", s), ("all_text", yes)],
        "assert" => &[
            ("page_contains", s),
            ("not_empty", s),
            ("count_min", count),
            ("url_contains", s),
        ],
        _ => &[],
    }
}

/// `(action, its value)` of a step.
fn split(step: &Step) -> (String, Value) {
    match serde_json::to_value(step) {
        Ok(Value::Object(m)) => m.into_iter().next().unwrap_or_default(),
        _ => (String::new(), Value::Null),
    }
}

fn field_label(key: &str) -> String {
    crate::api::error_i18n::resolve(&format!("web-sites-field-{}", key.replace('_', "-")), None)
}

/// Form of the selected step.
#[component]
pub(super) fn StepInspector(
    recipe: Signal<Recipe>,
    index: usize,
    can_write: bool,
    on_change: EventHandler<()>,
    on_delete: EventHandler<()>,
) -> Element {
    let mut recipe = recipe;
    let Some(step) = recipe.read().steps.get(index).cloned() else {
        return rsx! {};
    };
    let (action, value) = split(&step);
    // Writes a new value of the step back (kept only if it is a valid step).
    let mut write = {
        let action = action.clone();
        move |v: Value| {
            if let Ok(s) = serde_json::from_value::<Step>(json!({ action.clone(): v })) {
                recipe.with_mut(|r| {
                    if let Some(slot) = r.steps.get_mut(index) {
                        *slot = s;
                    }
                });
                on_change.call(());
            }
        }
    };
    let alts = alternatives(&action);
    rsx! {
        div { class: "space-y-3 p-4",
            match value.clone() {
                // `open` and `scroll` hold a single string.
                Value::String(text) => rsx! {
                    label { class: "block text-xs text-gray-600", {field_label(&action)} }
                    input {
                        class: INPUT,
                        disabled: !can_write,
                        value: "{text}",
                        oninput: move |e| write(json!(e.value())),
                    }
                    p { class: FIELD_HELP, {crate::api::error_i18n::resolve(&format!("web-sites-help-{action}"), None)} }
                },
                Value::Object(map) => rsx! {
                    ObjectFields {
                        map: map.clone(),
                        alts: alts.iter().map(|(k, _)| k.to_string()).collect::<Vec<_>>(),
                        action: action.clone(),
                        can_write,
                        on_write: move |v| write(v),
                    }
                },
                _ => rsx! {},
            }
            if can_write {
                button { class: BTN_DANGER, onclick: move |_| on_delete.call(()),
                    {t!("web-sites-delete-step")}
                }
            }
        }
    }
}

/// Fields of an object step: the chosen alternative, its value, then the
/// common `name` / `timeout_ms` / `as` / `optional`.
#[component]
fn ObjectFields(
    map: Map<String, Value>,
    alts: Vec<String>,
    action: String,
    can_write: bool,
    on_write: EventHandler<Value>,
) -> Element {
    let chosen = alts
        .iter()
        .find(|k| map.contains_key(*k))
        .cloned()
        .unwrap_or_default();
    let set = {
        let map = map.clone();
        move |key: &str, v: Value| {
            let mut m = map.clone();
            if v.is_null() {
                m.remove(key);
            } else {
                m.insert(key.to_string(), v);
            }
            on_write.call(Value::Object(m));
        }
    };
    let pick = {
        let map = map.clone();
        let alts = alts.clone();
        let action = action.clone();
        move |key: String| {
            let mut m = map.clone();
            for k in &alts {
                m.remove(k);
            }
            let empty = alternatives(&action)
                .iter()
                .find(|(k, _)| *k == key)
                .map(|(_, f)| f())
                .unwrap_or(Value::Null);
            m.insert(key, empty);
            on_write.call(Value::Object(m));
        }
    };
    let current = map.get(&chosen).cloned().unwrap_or(Value::Null);
    let extras: Vec<&str> = match action.as_str() {
        "click" if chosen == "role" => vec!["name", "optional"],
        "click" => vec!["optional"],
        "wait" if chosen == "text" => vec!["timeout_ms", "optional"],
        "wait" => vec!["optional"],
        "follow_pdf" | "extract" => vec!["as", "optional"],
        _ => vec![],
    };
    rsx! {
        if !alts.is_empty() {
            label { class: "block text-xs text-gray-600", {t!("web-sites-field-kind")} }
            select {
                class: INPUT,
                disabled: !can_write,
                onchange: {
                    let pick = pick.clone();
                    move |e: FormEvent| pick(e.value())
                },
                for k in alts.clone() {
                    option { key: "{k}", value: "{k}", selected: k == chosen, {field_label(&k)} }
                }
            }
        }
        ValueField {
            key_name: chosen.clone(),
            value: current,
            can_write,
            on_set: {
                let set = set.clone();
                let chosen = chosen.clone();
                move |v| set(&chosen, v)
            },
        }
        for extra in extras {
            ValueField {
                key: "{extra}",
                key_name: extra.to_string(),
                value: map.get(extra).cloned().unwrap_or(Value::Null),
                can_write,
                on_set: {
                    let set = set.clone();
                    move |v| set(extra, v)
                },
            }
        }
    }
}

/// One field: text, number, pair of texts, flag or `{selector, min}`.
#[component]
fn ValueField(
    key_name: String,
    value: Value,
    can_write: bool,
    on_set: EventHandler<Value>,
) -> Element {
    let label = field_label(&key_name);
    let numeric = matches!(key_name.as_str(), "ms" | "timeout_ms");
    let flag = matches!(key_name.as_str(), "optional" | "all_text");
    rsx! {
        if flag {
            label { class: "inline-flex items-center gap-2 text-sm",
                input {
                    r#type: "checkbox",
                    disabled: !can_write || key_name == "all_text",
                    checked: value.as_bool().unwrap_or(false),
                    onchange: move |e| on_set.call(if e.checked() { json!(true) } else { Value::Null }),
                }
                "{label}"
            }
        } else if key_name == "between" {
            label { class: "block text-xs text-gray-600", "{label}" }
            div { class: "grid grid-cols-2 gap-2",
                for side in 0..2usize {
                    input {
                        key: "{side}",
                        class: INPUT,
                        disabled: !can_write,
                        value: "{value[side].as_str().unwrap_or_default()}",
                        oninput: {
                            let value = value.clone();
                            move |e: FormEvent| {
                                let mut v = value.clone();
                                v[side] = json!(e.value());
                                on_set.call(v);
                            }
                        },
                    }
                }
            }
        } else if key_name == "count_min" {
            label { class: "block text-xs text-gray-600", "{label}" }
            div { class: "grid grid-cols-3 gap-2",
                input {
                    class: "{INPUT} col-span-2",
                    disabled: !can_write,
                    value: "{value[\"selector\"].as_str().unwrap_or_default()}",
                    oninput: {
                        let value = value.clone();
                        move |e: FormEvent| {
                            let mut v = value.clone();
                            v["selector"] = json!(e.value());
                            on_set.call(v);
                        }
                    },
                }
                input {
                    class: INPUT,
                    r#type: "number",
                    min: "0",
                    disabled: !can_write,
                    value: "{value[\"min\"].as_u64().unwrap_or(1)}",
                    oninput: {
                        let value = value.clone();
                        move |e: FormEvent| {
                            if let Ok(n) = e.value().parse::<u32>() {
                                let mut v = value.clone();
                                v["min"] = json!(n);
                                on_set.call(v);
                            }
                        }
                    },
                }
            }
        } else {
            label { class: "block text-xs text-gray-600", "{label}" }
            input {
                class: INPUT,
                r#type: if numeric { "number" } else { "text" },
                disabled: !can_write,
                value: "{value.as_str().map(str::to_string).or_else(|| value.as_u64().map(|n| n.to_string())).unwrap_or_default()}",
                oninput: move |e| {
                    let raw = e.value();
                    if numeric {
                        match raw.parse::<u32>() {
                            Ok(n) => on_set.call(json!(n)),
                            Err(_) if raw.is_empty() => on_set.call(Value::Null),
                            Err(_) => {}
                        }
                    } else if raw.is_empty() && matches!(key_name.as_str(), "as" | "name") {
                        on_set.call(Value::Null);
                    } else {
                        on_set.call(json!(raw));
                    }
                },
            }
        }
    }
}

/// Settings of the rule itself (name, site, parameters, hosts, browser).
#[component]
pub(super) fn RecipeSettings(
    recipe: Signal<Recipe>,
    can_write: bool,
    on_change: EventHandler<()>,
) -> Element {
    let mut recipe = recipe;
    let r = recipe.read().clone();
    let list = |v: &[String]| v.join(", ");
    let parse = |s: String| -> Vec<String> {
        s.split(',')
            .map(str::trim)
            .filter(|x| !x.is_empty())
            .map(str::to_string)
            .collect()
    };
    rsx! {
        div { class: "space-y-3 p-4",
            label { class: "block text-xs text-gray-600", {t!("web-sites-field-rule-name")} }
            input {
                class: INPUT,
                disabled: !can_write,
                value: "{r.name}",
                oninput: move |e| {
                    recipe.with_mut(|r| r.name = e.value());
                    on_change.call(());
                },
            }
            label { class: "block text-xs text-gray-600", {t!("web-sites-field-site")} }
            input {
                class: INPUT,
                disabled: !can_write,
                value: "{r.site}",
                oninput: move |e| {
                    recipe.with_mut(|r| r.site = e.value());
                    on_change.call(());
                },
            }
            label { class: "block text-xs text-gray-600", {t!("web-sites-field-params")} }
            input {
                class: INPUT,
                disabled: !can_write,
                value: "{list(&r.params)}",
                oninput: move |e| {
                    recipe.with_mut(|r| r.params = parse(e.value()));
                    on_change.call(());
                },
            }
            p { class: FIELD_HELP, {t!("web-sites-help-params")} }
            label { class: "block text-xs text-gray-600", {t!("web-sites-field-allow-hosts")} }
            input {
                class: INPUT,
                disabled: !can_write,
                value: "{list(&r.allow_hosts)}",
                oninput: move |e| {
                    recipe.with_mut(|r| r.allow_hosts = parse(e.value()));
                    on_change.call(());
                },
            }
            label { class: "block text-xs text-gray-600", {t!("web-sites-field-browser")} }
            input {
                class: INPUT,
                disabled: !can_write,
                value: "{r.browser.clone().unwrap_or_default()}",
                oninput: move |e| {
                    let v = e.value();
                    recipe.write().browser = (!v.trim().is_empty()).then(|| v.trim().to_string());
                    on_change.call(());
                },
            }
            p { class: FIELD_HELP, {t!("web-sites-help-browser")} }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn recipe(n: usize) -> Recipe {
        Recipe {
            name: "r".into(),
            site: "x.fr".into(),
            allow_hosts: vec![],
            params: vec![],
            browser: None,
            steps: (0..n).map(|i| Step::Open(format!("/{i}"))).collect(),
        }
    }

    #[test]
    fn drops_insert_and_reorder() {
        let mut r = recipe(3);
        assert_eq!(drop_at(&mut r, Drag::Move(0), 3), 2);
        assert_eq!(r.steps[2], Step::Open("/0".into()));
        assert_eq!(drop_at(&mut r, Drag::Move(2), 0), 0);
        assert_eq!(r.steps[0], Step::Open("/0".into()));
        assert_eq!(drop_at(&mut r, Drag::New("scroll"), 1), 1);
        assert_eq!(r.steps[1].action(), "scroll");
        assert_eq!(r.steps.len(), 4);
    }
}
