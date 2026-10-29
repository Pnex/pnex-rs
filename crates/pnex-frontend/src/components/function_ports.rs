//! Editable port grids for the function editor. The interface is structured
//! data of the version (D497), never text in the code: every cell edit
//! produces a new `FunctionSignature` through `onedit`.

use dioxus::prelude::*;
use pnex_core::{FunctionInput, FunctionOutput, FunctionSignature, FunctionType};

use dioxus_i18n::t;

/// Default typed in a cell → value of the declared type. Text that does not
/// fit the type is kept as a string: the cell turns red and the server
/// refuses it (`inputs.<i>.default`).
pub fn parse_default(ty: FunctionType, text: &str) -> Option<serde_json::Value> {
    let text = text.trim();
    if text.is_empty() {
        return None;
    }
    let raw = || serde_json::Value::String(text.to_string());
    Some(match ty {
        FunctionType::Number => text
            .parse::<serde_json::Number>()
            .map(serde_json::Value::Number)
            .unwrap_or_else(|_| raw()),
        FunctionType::Bool => match text {
            "true" => serde_json::Value::Bool(true),
            "false" => serde_json::Value::Bool(false),
            _ => raw(),
        },
        FunctionType::String => raw(),
        FunctionType::Any => serde_json::from_str(text).unwrap_or_else(|_| raw()),
    })
}

/// Default shown in its cell (strings without quotes).
fn default_text(v: &serde_json::Value) -> String {
    match v {
        serde_json::Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

/// Token of a declared type (select values).
fn type_token(ty: FunctionType) -> &'static str {
    match ty {
        FunctionType::Number => "number",
        FunctionType::String => "string",
        FunctionType::Bool => "bool",
        FunctionType::Any => "any",
    }
}

fn type_of(token: &str) -> FunctionType {
    match token {
        "string" => FunctionType::String,
        "bool" => FunctionType::Bool,
        "any" => FunctionType::Any,
        _ => FunctionType::Number,
    }
}

/// First free `<base>_<n>` name among the declared ports.
pub fn free_port_name(sig: &FunctionSignature, base: &str) -> String {
    let taken =
        |c: &str| sig.inputs.iter().any(|i| i.name == c) || sig.outputs.iter().any(|o| o.name == c);
    (0..)
        .map(|n| format!("{base}_{n}"))
        .find(|c| !taken(c))
        .expect("an unbounded range")
}

/// Port edit applied to the signature: (is_input, row index, change).
#[derive(Debug, Clone, PartialEq)]
pub enum PortEdit {
    Name(String),
    Type(FunctionType),
    Default(String),
    Desc(String),
    Delete,
}

/// The signature with one row edited.
pub fn edit_port(
    sig: &FunctionSignature,
    is_input: bool,
    row: usize,
    edit: PortEdit,
) -> FunctionSignature {
    let mut next = sig.clone();
    let desc = |s: String| Some(s).filter(|s| !s.trim().is_empty());
    if is_input {
        if row >= next.inputs.len() {
            return next;
        }
        let port = &mut next.inputs[row];
        match edit {
            PortEdit::Name(v) => port.name = v,
            PortEdit::Type(ty) => {
                port.ty = ty;
                port.default = port
                    .default
                    .as_ref()
                    .and_then(|d| parse_default(ty, &default_text(d)));
            }
            PortEdit::Default(v) => port.default = parse_default(port.ty, &v),
            PortEdit::Desc(v) => port.desc = desc(v),
            PortEdit::Delete => {
                next.inputs.remove(row);
            }
        }
    } else {
        if row >= next.outputs.len() {
            return next;
        }
        let port = &mut next.outputs[row];
        match edit {
            PortEdit::Name(v) => port.name = v,
            PortEdit::Type(ty) => port.ty = ty,
            PortEdit::Default(_) => {}
            PortEdit::Desc(v) => port.desc = desc(v),
            PortEdit::Delete => {
                next.outputs.remove(row);
            }
        }
    }
    next
}

/// The signature with a new port named `name` (type `ty`).
pub fn add_port(
    sig: &FunctionSignature,
    is_input: bool,
    name: String,
    ty: FunctionType,
) -> FunctionSignature {
    let mut next = sig.clone();
    if is_input {
        next.inputs.push(FunctionInput {
            name,
            ty,
            default: None,
            desc: None,
        });
    } else {
        next.outputs.push(FunctionOutput {
            name,
            ty,
            desc: None,
        });
    }
    next
}

// ───────────────────────── Panel component ─────────────────────────

/// Inputs and outputs grids. No output = message mode (one output, the
/// returned value is the message); adding an output switches to named ports.
#[component]
pub fn FunctionPortsPanel(
    sig: FunctionSignature,
    readonly: bool,
    onedit: EventHandler<FunctionSignature>,
) -> Element {
    rsx! {
        PortsSection {
            sig: sig.clone(),
            is_input: true,
            readonly,
            onedit,
        }
        PortsSection {
            sig,
            is_input: false,
            readonly,
            onedit,
        }
    }
}

/// One grid (inputs: name / type / default / label / delete; outputs: no
/// default).
#[component]
fn PortsSection(
    sig: FunctionSignature,
    is_input: bool,
    readonly: bool,
    onedit: EventHandler<FunctionSignature>,
) -> Element {
    let rows: Vec<(String, FunctionType, Option<String>, Option<String>)> = if is_input {
        sig.inputs
            .iter()
            .map(|i| {
                (
                    i.name.clone(),
                    i.ty,
                    Some(i.default.as_ref().map(default_text).unwrap_or_default()),
                    i.desc.clone(),
                )
            })
            .collect()
    } else {
        sig.outputs
            .iter()
            .map(|o| (o.name.clone(), o.ty, None, o.desc.clone()))
            .collect()
    };
    let bad_default: Vec<bool> = if is_input {
        sig.inputs
            .iter()
            .map(|i| i.default.as_ref().is_some_and(|d| !i.ty.accepts(d)))
            .collect()
    } else {
        vec![false; rows.len()]
    };
    let grid = if is_input {
        "grid grid-cols-[112px_84px_64px_minmax(0,1fr)_28px] gap-1.5 items-center"
    } else {
        "grid grid-cols-[112px_84px_minmax(0,1fr)_28px] gap-1.5 items-center"
    };
    let title = if is_input {
        t!("functions-ports-in-title")
    } else {
        t!("functions-ports-out-title")
    };
    let dot = if is_input {
        "border-teal-700"
    } else {
        "border-amber-600"
    };
    let message_mode = !is_input && rows.is_empty();
    let count = rows.len();
    rsx! {
        section { class: "rounded-xl border border-gray-200 bg-white p-3.5 flex flex-col gap-2",
            div { class: "flex items-center gap-2",
                span { class: "w-2.5 h-2.5 rounded-full border-2 {dot}" }
                h3 { class: "text-base font-semibold", {title} }
                span { class: "text-xs text-gray-600 bg-gray-100 rounded-full px-2",
                    "{count}"
                }
                if !is_input {
                    span { class: "ml-auto text-xs font-medium text-gray-600",
                        if message_mode {
                            {t!("functions-mode-message")}
                        } else {
                            {t!("functions-mode-ports")}
                        }
                    }
                }
            }
            if message_mode {
                p { class: "text-sm text-gray-500", {t!("functions-ports-none")} }
            } else if count > 0 {
                div { class: "{grid} text-xs text-gray-600",
                    span { {t!("functions-ports-name")} }
                    span { {t!("functions-ports-type")} }
                    if is_input {
                        span { {t!("functions-ports-default")} }
                    }
                    span { {t!("functions-ports-label")} }
                    span {}
                }
            }
            for (row, (name, ty, default, desc)) in rows.into_iter().enumerate() {
                PortRow {
                    key: "{row}",
                    sig: sig.clone(),
                    is_input,
                    row,
                    name,
                    ty,
                    default,
                    desc,
                    bad_default: bad_default[row],
                    readonly,
                    onedit,
                }
            }
            if !readonly {
                AddPortButton { sig, is_input, onedit }
            }
        }
    }
}

/// One port row; each cell emits the edited signature.
#[component]
fn PortRow(
    sig: FunctionSignature,
    is_input: bool,
    row: usize,
    name: String,
    ty: FunctionType,
    default: Option<String>,
    desc: Option<String>,
    bad_default: bool,
    readonly: bool,
    onedit: EventHandler<FunctionSignature>,
) -> Element {
    let cell = "h-8 rounded-md border border-gray-300 px-2 font-mono text-sm min-w-0";
    let cell_bad =
        "h-8 rounded-md border-2 border-red-400 bg-red-50 px-2 font-mono text-sm min-w-0";
    let name_class = if pnex_core::is_valid_port_name(&name) {
        cell
    } else {
        cell_bad
    };
    let default_class = if bad_default { cell_bad } else { cell };
    let grid = if is_input {
        "grid grid-cols-[112px_84px_64px_minmax(0,1fr)_28px] gap-1.5 items-center"
    } else {
        "grid grid-cols-[112px_84px_minmax(0,1fr)_28px] gap-1.5 items-center"
    };
    let emit = move |edit: PortEdit| onedit.call(edit_port(&sig, is_input, row, edit));
    // One clone of the emitter per cell handler.
    let (on_name, on_type, on_default, on_desc, on_delete) =
        (emit.clone(), emit.clone(), emit.clone(), emit.clone(), emit);
    rsx! {
        div { class: "{grid}",
            input {
                class: "{name_class}",
                value: "{name}",
                disabled: readonly,
                oninput: move |ev| on_name(PortEdit::Name(ev.value())),
            }
            select {
                class: "{cell}",
                value: "{type_token(ty)}",
                disabled: readonly,
                onchange: move |ev| on_type(PortEdit::Type(type_of(&ev.value()))),
                option { value: "number", "number" }
                option { value: "string", "string" }
                option { value: "bool", "bool" }
                option { value: "any", "any" }
            }
            if let Some(d) = default {
                input {
                    class: "{default_class}",
                    value: "{d}",
                    disabled: readonly,
                    placeholder: "—",
                    oninput: move |ev| on_default(PortEdit::Default(ev.value())),
                }
            }
            input {
                class: "{cell} not-mono",
                value: desc.unwrap_or_default(),
                disabled: readonly,
                oninput: move |ev| on_desc(PortEdit::Desc(ev.value())),
            }
            button {
                class: "text-gray-500 hover:text-red-600 text-lg leading-none",
                disabled: readonly,
                title: t!("functions-ports-delete-title"),
                onclick: move |_| on_delete(PortEdit::Delete),
                "×"
            }
        }
    }
}

/// Dashed add-port button.
#[component]
fn AddPortButton(
    sig: FunctionSignature,
    is_input: bool,
    onedit: EventHandler<FunctionSignature>,
) -> Element {
    let add_label = if is_input {
        t!("functions-ports-add-in")
    } else {
        t!("functions-ports-add-out")
    };
    rsx! {
        button {
            class: "h-9 rounded-lg border border-dashed border-gray-300 bg-white text-sm font-medium text-signal-700 hover:bg-signal-50",
            onclick: move |_| {
                let name = free_port_name(&sig, if is_input { "input" } else { "output" });
                onedit.call(add_port(&sig, is_input, name, FunctionType::Number));
            },
            {add_label}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn edits_apply_to_the_signature() {
        let sig = add_port(
            &FunctionSignature::default(),
            true,
            "t".into(),
            FunctionType::Number,
        );
        let sig = edit_port(&sig, true, 0, PortEdit::Default("20".into()));
        assert_eq!(sig.inputs[0].default, Some(json!(20)));
        // A default that does not fit the type stays visible (red cell).
        let bad = edit_port(&sig, true, 0, PortEdit::Default("abc".into()));
        assert!(!bad.inputs[0]
            .ty
            .accepts(bad.inputs[0].default.as_ref().unwrap()));
        // Type change converts the default when it can.
        let s = edit_port(&sig, true, 0, PortEdit::Type(FunctionType::String));
        assert_eq!(s.inputs[0].default, Some(json!("20")));
        let s = edit_port(&sig, true, 0, PortEdit::Desc("  ".into()));
        assert_eq!(s.inputs[0].desc, None);
        assert!(edit_port(&sig, true, 0, PortEdit::Delete).inputs.is_empty());
    }

    #[test]
    fn outputs_switch_message_and_ports_mode() {
        let sig = FunctionSignature::default();
        assert!(sig.is_message());
        let name = free_port_name(&sig, "output");
        let ports = add_port(&sig, false, name, FunctionType::Any);
        assert_eq!(ports.outputs[0].name, "output_0");
        assert!(!ports.is_message());
        assert_eq!(free_port_name(&ports, "output"), "output_1");
        assert!(edit_port(&ports, false, 0, PortEdit::Delete).is_message());
    }
}
