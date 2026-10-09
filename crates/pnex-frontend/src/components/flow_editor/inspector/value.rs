use super::helpers::*;
use super::*;

#[component]
pub(super) fn CalcForm(mut cx: EditorCx, initial: CalcConfig, can_write: bool) -> Element {
    let mut expression = use_signal(move || initial.expression.clone());
    let errors = use_memo(move || pnex_core::validate_calc(&expression.cloned()));

    rsx! {
        div { class: "space-y-3",
            label { class: "block",
                span { class: "text-xs font-medium text-gray-500 mb-1 block",
                    {t!("flows-calc-expression")}
                }
                textarea {
                    class: if !errors.cloned().is_empty() { "w-full h-20 px-2 py-1.5 border border-red-400 bg-red-50 rounded-lg text-sm font-mono" } else { "w-full h-20 px-2 py-1.5 border border-gray-300 rounded-lg text-sm font-mono" },
                    value: "{expression}",
                    disabled: !can_write,
                    oninput: move |event| {
                        let raw = event.value();
                        expression.set(raw.clone());
                        patch_selected(
                            &mut cx,
                            move |node: &mut FlowNode| {
                                if let FlowNodeKind::Calc { config } = &mut node.kind {
                                    config.expression = raw;
                                }
                            },
                        );
                    },
                }
            }
            if !errors.cloned().is_empty() {
                div { class: "rounded-lg bg-red-50 border border-red-200 p-2 space-y-1",
                    for e in errors.cloned() {
                        p { class: "text-xs text-red-700", {e.to_string()} }
                    }
                }
            }
            if !expression.cloned().trim().is_empty() && errors.cloned().is_empty() {
                p { class: "text-xs text-gray-500",
                    {
                        format!(
                            "{} : {}",
                            t!("flows-calc-vars"),
                            pnex_core::calc_variables(&expression.cloned()).join(", "),
                        )
                    }
                }
            }
            p { class: "text-xs text-gray-400", {t!("flows-calc-functions-help")} }
        }
    }
}

// ─────────────── value ───────────────

/// Stored value back to editable raw text (round-trip stable: a stored
/// value re-renders the exact same raw input).
fn value_json_to_raw(v: &serde_json::Value) -> String {
    match v {
        serde_json::Value::Null => String::new(),
        serde_json::Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

/// Editor pane of the Json Values node — key/value rows for non-developers,
/// raw JSON for everyone else.
#[derive(Clone, Copy, PartialEq)]
enum ValuePane {
    Kv,
    Json,
}

#[component]
pub(super) fn ValueForm(mut cx: EditorCx, initial: ValueConfig, can_write: bool) -> Element {
    // Two panes: key/value rows (non-developers) and raw JSON with syntax
    // coloring. Strict parse in the JSON pane: the config only moves on a valid
    // document, the red hint flags invalid input.
    let starts_object = initial.value.as_object().is_some();
    let initial_value = initial.value.clone();
    let initial_raw = value_json_to_raw(&initial.value);
    let initial_rows = initial
        .value
        .as_object()
        .map(|o| {
            o.iter()
                .map(|(k, v)| (k.clone(), value_json_to_raw(v)))
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let mut pane = use_signal(move || {
        if starts_object {
            ValuePane::Kv
        } else {
            ValuePane::Json
        }
    });
    let mut value = use_signal(move || initial_value.clone());
    let mut raw = use_signal(move || initial_raw.clone());
    let mut invalid = use_signal(move || false);
    let mut rows = use_signal(move || initial_rows.clone());

    rsx! {
        div { class: "space-y-3",
            p { class: "text-xs text-gray-500", {t!("flows-value-help")} }

            // ── Pane toggle: key/value (non-developers) vs raw JSON ──
            div { class: "flex gap-1",
                button {
                    class: if pane() == ValuePane::Kv { "px-2 py-0.5 rounded-full text-xs bg-blue-600 text-white" } else { "px-2 py-0.5 rounded-full text-xs bg-gray-100 text-gray-600 hover:bg-gray-200" },
                    disabled: !can_write,
                    onclick: move |_| {
                        pane.set(ValuePane::Kv);
                    },
                    {t!("flows-value-pane-kv")}
                }
                button {
                    class: if pane() == ValuePane::Json { "px-2 py-0.5 rounded-full text-xs bg-blue-600 text-white" } else { "px-2 py-0.5 rounded-full text-xs bg-gray-100 text-gray-600 hover:bg-gray-200" },
                    disabled: !can_write,
                    onclick: move |_| {
                        raw.set(value_json_to_raw(&value.read().clone()));
                        invalid.set(false);
                        pane.set(ValuePane::Json);
                    },
                    {t!("flows-value-pane-json")}
                }
            }

            match pane() {
                ValuePane::Json => rsx! {
                    JsonEditor {
                        value: "{raw}",
                        readonly: !can_write,
                        placeholder: "{ \"key\": \"value\" }".to_string(),
                        height_class: "h-32".to_string(),
                        oninput: move |event: String| {
                            let parsed: Option<serde_json::Value> = if event.trim().is_empty() {
                                Some(serde_json::Value::Null)
                            } else {
                                serde_json::from_str(event.trim()).ok()
                            };
                            invalid.set(parsed.is_none());
                            raw.set(event.clone());
                            if let Some(v) = parsed {
                                value.set(v.clone());
                                patch_value_config(&mut cx, v);
                            }
                        },
                    }
                    if invalid() {
                        p { class: "text-xs text-red-500", {t!("flows-value-invalid-json")} }
                    }
                    p { class: "text-xs text-gray-400", {t!("flows-value-static-help")} }
                },
                ValuePane::Kv => rsx! {
                    if value.read().as_object().is_none() {
                        p { class: "text-xs text-gray-500", {t!("flows-value-kv-non-object")} }
                    } else {
                        div { class: "space-y-1",
                            for (idx, (key, cell)) in rows.read().clone().into_iter().enumerate() {
                                div { class: "flex items-center gap-1",
                                    input {
                                        class: "w-2/5 px-2 py-1 border border-gray-300 rounded-lg text-sm font-mono",
                                        value: "{key}",
                                        disabled: !can_write,
                                        placeholder: "key",
                                        oninput: move |event| {
                                            let text = event.value();
                                            let mut next = rows.read().clone();
                                            if idx < next.len() {
                                                next[idx].0 = text;
                                            }
                                            rows.set(next.clone());
                                            patch_value_config(&mut cx, rows_to_object(&next));
                                        },
                                    }
                                    span { class: "text-xs text-gray-400", "=" }
                                    input {
                                        class: "flex-1 px-2 py-1 border border-gray-300 rounded-lg text-sm",
                                        value: "{cell}",
                                        disabled: !can_write,
                                        placeholder: "value",
                                        oninput: move |event| {
                                            let text = event.value();
                                            let mut next = rows.read().clone();
                                            if idx < next.len() {
                                                next[idx].1 = text;
                                            }
                                            rows.set(next.clone());
                                            patch_value_config(&mut cx, rows_to_object(&next));
                                        },
                                    }
                                    if can_write {
                                        button {
                                            class: "text-gray-300 hover:text-red-500 text-sm px-1",
                                            onclick: move |_| {
                                                let mut next = rows.read().clone();
                                                if idx < next.len() {
                                                    next.remove(idx);
                                                }
                                                rows.set(next.clone());
                                                patch_value_config(&mut cx, rows_to_object(&next));
                                            },
                                            "×"
                                        }
                                    }
                                }
                            }
                            if can_write {
                                button {
                                    class: "text-xs text-blue-600 hover:text-blue-700",
                                    onclick: move |_| {
                                        let mut next = rows.read().clone();
                                        next.push((String::new(), String::new()));
                                        rows.set(next.clone());
                                        patch_value_config(&mut cx, rows_to_object(&next));
                                    },
                                    {t!("flows-value-kv-add")}
                                }
                            }
                        }
                    }
                },
            }
        }
    }
}

/// Rebuilds the JSON object from the key/value rows — empty keys are
/// skipped (a half-typed row must not wipe the document).
fn rows_to_object(rows: &[(String, String)]) -> serde_json::Value {
    serde_json::Value::Object(
        rows.iter()
            .filter(|(k, _)| !k.trim().is_empty())
            .map(|(k, cell)| (k.clone(), kv_cell_to_json(cell)))
            .collect(),
    )
}

/// Key/value pane: a cell parses as JSON when it can (numbers, booleans,
/// null), anything else is a plain string — non-developers never type
/// quotes.
fn kv_cell_to_json(text: &str) -> serde_json::Value {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return serde_json::Value::String(String::new());
    }
    serde_json::from_str(trimmed).unwrap_or(serde_json::Value::String(text.to_string()))
}

/// Commits a JSON document to the selected Json Values node.
fn patch_value_config(cx: &mut EditorCx, v: serde_json::Value) {
    patch_selected(cx, move |node: &mut FlowNode| {
        if let FlowNodeKind::Value { config } = &mut node.kind {
            config.value = v.clone();
        }
    });
}
