//! Key/value pills editor — replaces the raw JSON textarea for device
//! metadata (`pnex-device-redesign.html` mockup: "Fields" pills by default,
//! JSON toggle as expert mode). Callback-driven (`on_save`): the parent owns
//! the API call (`PATCH /devices/{id}` metadata-only contract).

use dioxus::prelude::*;
use dioxus_i18n::t;
use serde_json::{Map, Value};

/// Current editor view.
#[derive(Clone, Copy, PartialEq)]
enum View {
    Fields,
    Json,
}

/// Key/value rows from a JSON object (non-string values stringified;
/// `nested` = at least one non-textual value → "JSON mode" hint).
fn rows_from_json(value: Value) -> (Vec<(String, String)>, bool) {
    let Some(map) = value.as_object() else {
        return (Vec::new(), false);
    };
    let mut rows = Vec::with_capacity(map.len());
    let mut nested = false;
    for (k, v) in map {
        let sv = match v {
            Value::String(s) => s.clone(),
            other => {
                nested = true;
                other.to_string()
            }
        };
        rows.push((k.clone(), sv));
    }
    (rows, nested)
}

/// Flat JSON object from rows (wizard `rows_to_json` semantics: trim,
/// string values, incomplete pairs skipped, `null` when empty).
fn json_from_rows(rows: &[(String, String)]) -> Value {
    let map: Map<String, Value> = rows
        .iter()
        .filter(|(k, v)| !k.trim().is_empty() && !v.trim().is_empty())
        .map(|(k, v)| (k.trim().to_string(), Value::String(v.trim().to_string())))
        .collect();
    if map.is_empty() {
        Value::Null
    } else {
        Value::Object(map)
    }
}

/// The editor renders the header (title + Fields/JSON toggle), the pills,
/// the add row and the save footer. `can_write = false` → read-only (pills
/// without remove buttons, no toggle, no footer).
#[component]
pub fn KvPillsEditor(
    /// Initial value (JSON object expected; non-object → JSON view forced).
    initial: Value,
    /// Title and hint, i18n resolved by the caller (CRUD kit doctrine).
    title: String,
    hint: String,
    can_write: bool,
    /// Save: the parent owns the request (metadata-only).
    on_save: Callback<Value>,
) -> Element {
    let forced_json = !initial.is_null() && !initial.is_object();
    let (initial_rows, initial_nested) = rows_from_json(initial.clone());
    let mut view = use_signal(move || {
        if forced_json {
            View::Json
        } else {
            View::Fields
        }
    });
    let mut rows: Signal<Vec<(String, String)>> = use_signal(move || initial_rows);
    let mut nested = use_signal(move || initial_nested);
    let mut raw = use_signal(move || serde_json::to_string_pretty(&initial).unwrap_or_default());
    let mut new_key = use_signal(String::new);
    let mut new_val = use_signal(String::new);
    let mut invalid = use_signal(|| false);

    // Precompute outside rsx (`let` is forbidden in conditional rsx blocks).
    let entries = rows.read().clone();
    rsx! {
        div { class: "space-y-3",
            // Header: title + Fields/JSON (expert) toggle.
            div { class: "flex items-center gap-3",
                h3 { class: "text-sm font-semibold text-gray-500 uppercase tracking-wider", "{title}" }
                if can_write && !forced_json {
                    div { class: "ml-auto flex overflow-hidden rounded-lg border border-gray-300 text-xs font-medium",
                        button {
                            class: if view() == View::Fields {
                                "bg-blue-50 px-3 py-1.5 text-blue-700"
                            } else {
                                "bg-white px-3 py-1.5 text-gray-500 hover:bg-gray-50"
                            },
                            onclick: move |_| {
                                // Back to fields: the JSON textarea must be
                                // valid to leave (otherwise stay on it).
                                if view() == View::Json {
                                    let parsed: Result<Value, _> = if raw.read().trim().is_empty() {
                                        Ok(Value::Null)
                                    } else {
                                        serde_json::from_str(&raw.read())
                                    };
                                    match parsed {
                                        Ok(value) => {
                                            let (next_rows, next_nested) = rows_from_json(value);
                                            rows.set(next_rows);
                                            nested.set(next_nested);
                                            invalid.set(false);
                                            view.set(View::Fields);
                                        }
                                        Err(_) => invalid.set(true),
                                    }
                                }
                            },
                            {t!("devices-labels-fields-tab")}
                        }
                        button {
                            class: if view() == View::Json {
                                "bg-blue-50 px-3 py-1.5 text-blue-700"
                            } else {
                                "bg-white px-3 py-1.5 text-gray-500 hover:bg-gray-50"
                            },
                            onclick: move |_| {
                                // Switch to JSON: current state (pills) is
                                // serialized as-is into the textarea.
                                if view() == View::Fields {
                                    let snapshot = rows.read().clone();
                                    if let Ok(text) =
                                        serde_json::to_string_pretty(&json_from_rows(&snapshot))
                                    {
                                        raw.set(text);
                                    }
                                    invalid.set(false);
                                    view.set(View::Json);
                                }
                            },
                            {t!("devices-labels-json-tab")}
                        }
                    }
                }
            }
            p { class: "text-xs text-gray-400", "{hint}" }
            if invalid() {
                p { class: "text-xs text-red-600", {t!("devices-labels-invalid")} }
            }
            if nested() && view() == View::Fields {
                p { class: "text-xs text-amber-600", {t!("devices-labels-nested-hint")} }
            }
            if view() == View::Fields {
                // Key/value pills (mockup) — the × removes the pair;
                // re-adding an existing key updates its value.
                if entries.is_empty() {
                    p { class: "text-sm italic text-gray-400", {t!("devices-labels-empty")} }
                }
                div { class: "flex flex-wrap gap-2",
                    for (k, v) in entries {
                        span {
                            key: "{k}",
                            class: "inline-flex items-stretch overflow-hidden rounded-full border border-gray-200 text-sm",
                            span {
                                class: "border-r border-gray-200 bg-gray-50 px-3 py-1.5 font-mono text-xs text-gray-600",
                                {k.clone()}
                            }
                            span { class: "break-all px-3 py-1.5 text-gray-800", {v.clone()} }
                            if can_write {
                                button {
                                    class: "px-2 text-gray-400 hover:text-red-600",
                                    title: t!("devices-labels-remove"),
                                    onclick: move |_| {
                                        rows.with_mut(|list| list.retain(|(ek, _)| ek != &k));
                                    },
                                    {"×"}
                                }
                            }
                        }
                    }
                }
                if can_write {
                    div { class: "flex flex-wrap items-center gap-2",
                        input {
                            class: "w-36 rounded-lg border border-gray-300 px-2.5 py-1.5 font-mono text-xs",
                            placeholder: t!("devices-labels-key-placeholder"),
                            value: "{new_key}",
                            oninput: move |evt| new_key.set(evt.value()),
                            onkeydown: move |evt: KeyboardEvent| {
                                if evt.key() == Key::Enter {
                                    add_pair(new_key, new_val, rows);
                                }
                            },
                        }
                        input {
                            class: "min-w-40 flex-1 rounded-lg border border-gray-300 px-2.5 py-1.5 text-sm",
                            placeholder: t!("devices-labels-value-placeholder"),
                            value: "{new_val}",
                            oninput: move |evt| new_val.set(evt.value()),
                            onkeydown: move |evt: KeyboardEvent| {
                                if evt.key() == Key::Enter {
                                    add_pair(new_key, new_val, rows);
                                }
                            },
                        }
                        button {
                            class: "rounded-lg bg-blue-600 px-3 py-1.5 text-sm font-medium text-white hover:bg-blue-700",
                            onclick: move |_| add_pair(new_key, new_val, rows),
                            {t!("devices-labels-add")}
                        }
                    }
                }
            } else {
                // JSON view (expert) — pretty-printed, red border when invalid.
                textarea {
                    class: if invalid() {
                        "h-40 w-full rounded-lg border border-red-400 bg-red-50 px-3 py-2 font-mono text-sm"
                    } else {
                        "h-40 w-full rounded-lg border border-gray-300 px-3 py-2 font-mono text-sm"
                    },
                    value: "{raw}",
                    spellcheck: false,
                    oninput: move |evt| {
                        raw.set(evt.value());
                        invalid.set(false);
                    },
                }
            }
            if can_write {
                div { class: "flex justify-end",
                    button {
                        class: "rounded-lg bg-blue-600 px-4 py-2 text-sm font-medium text-white hover:bg-blue-700",
                        onclick: move |_| {
                            if view() == View::Json {
                                let parsed: Result<Value, _> = if raw.read().trim().is_empty() {
                                    Ok(Value::Null)
                                } else {
                                    serde_json::from_str(&raw.read())
                                };
                                match parsed {
                                    Ok(value) => on_save.call(value),
                                    Err(_) => invalid.set(true),
                                }
                            } else {
                                let snapshot = rows.read().clone();
                                on_save.call(json_from_rows(&snapshot));
                            }
                        },
                        {t!("devices-labels-save")}
                    }
                }
            }
        }
    }
}

/// Add/update a pair from the add row (trim; empty key ignored; existing
/// key = value update, per the mockup).
fn add_pair(
    mut key: Signal<String>,
    mut val: Signal<String>,
    mut rows: Signal<Vec<(String, String)>>,
) {
    let k = key.read().trim().to_string();
    let v = val.read().trim().to_string();
    if k.is_empty() {
        return;
    }
    let mut next = rows.read().clone();
    if let Some(slot) = next.iter_mut().find(|(ek, _)| *ek == k) {
        slot.1 = v;
    } else {
        next.push((k, v));
    }
    rows.set(next);
    key.set(String::new());
    val.set(String::new());
}
