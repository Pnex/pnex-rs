//! Editable port grids for the function editor — the code stays the single
//! source of truth: every cell edit REWRITES the port's directive line in
//! the code (no backend schema change). Uses `pnex_core::directive_spans`
//! to locate each directive line.

use dioxus::prelude::*;
use pnex_core::{FunctionInput, FunctionOutput, FunctionSignature, FunctionType};

use dioxus_i18n::t;

/// Fields of one `@input`/`@output` directive line.
#[derive(Debug, Clone, PartialEq)]
pub struct DirectiveFields {
    pub name: String,
    pub ty: FunctionType,
    pub default: Option<String>,
    pub desc: Option<String>,
}

/// Port name rule — mirrors the pnex-core `is_valid_name` rule.
pub fn is_valid_port_name(name: &str) -> bool {
    let mut chars = name.chars();
    match chars.next() {
        Some(c) if c == '_' || c.is_ascii_lowercase() => {
            chars.all(|c| c == '_' || c.is_ascii_alphanumeric())
        }
        _ => false,
    }
}

/// Replaces ONLY the directive line at `line` (1-based) with the canonical
/// rendering of `fields`. Returns None when the line is not the expected
/// directive (safety net against stale grid keys).
pub fn set_directive_fields(
    code: &str,
    line: u32,
    is_input: bool,
    fields: &DirectiveFields,
) -> Option<String> {
    let mut lines: Vec<&str> = code.split('\n').collect();
    let idx = (line - 1) as usize;
    if idx >= lines.len() {
        return None;
    }
    // Safety net: the line must still be a directive of the expected kind
    // (stale grid key guard). Renames rely on the line identity only.
    let expected = if is_input { "@input" } else { "@output" };
    if !lines[idx].contains(expected) {
        return None;
    }
    let prefix = comment_prefix(lines[idx]);
    let rendered = render_directive(is_input, fields, &prefix);
    lines[idx] = &rendered;
    Some(lines.join("\n"))
}

/// Canonical rendering `// @input name type[=default] "desc"` with the
/// line's own comment prefix (`//` or `#`).
fn render_directive(is_input: bool, f: &DirectiveFields, prefix: &str) -> String {
    let word = if is_input { "@input" } else { "@output" };
    let mut s = format!("{prefix} {word} {} {}", f.name, type_token(f.ty));
    if is_input {
        if let Some(d) = &f.default {
            let d = d.trim();
            if !d.is_empty() {
                s.push('=');
                s.push_str(d);
            }
        }
    }
    if let Some(desc) = &f.desc {
        let desc = desc.trim();
        if !desc.is_empty() {
            s.push_str(&format!(" \"{desc}\""));
        }
    }
    s
}

/// Comment prefix of an existing directive line (`//` or `#`).
fn comment_prefix(line: &str) -> String {
    let t = line.trim_start();
    if t.starts_with("//") {
        "//".to_string()
    } else {
        "#".to_string()
    }
}

/// Token of a declared type in directives.
fn type_token(ty: FunctionType) -> &'static str {
    match ty {
        FunctionType::Number => "number",
        FunctionType::String => "string",
        FunctionType::Bool => "bool",
        FunctionType::Any => "any",
    }
}

/// Inserts a new directive line after the last directive (or at the very
/// top when none exists).
pub fn insert_directive(code: &str, is_input: bool, fields: &DirectiveFields) -> String {
    let last = pnex_core::directive_spans(code)
        .last()
        .map(|s| s.line)
        .unwrap_or(0);
    let line = render_directive(
        is_input,
        fields,
        if code.trim_start().starts_with('#') && !code.trim_start().starts_with("//") {
            "#"
        } else {
            "//"
        },
    );
    let mut lines: Vec<String> = code.split('\n').map(String::from).collect();
    lines.insert(last as usize, line);
    lines.join("\n")
}

/// Removes the directive line at `line` (1-based).
pub fn delete_directive(code: &str, line: u32) -> String {
    code.split('\n')
        .enumerate()
        .filter(|(idx, _)| (*idx as u32) + 1 != line)
        .map(|(_, l)| l)
        .collect::<Vec<_>>()
        .join("\n")
}
// ───────────────────────── Panel component ─────────────────────────

/// Editable port grids — mockup layout. Every cell edit rewrites the
/// directive line in `code` (single source of truth) via `onedit`.
#[component]
pub fn FunctionPortsPanel(
    code: String,
    sig: FunctionSignature,
    readonly: bool,
    onedit: EventHandler<String>,
) -> Element {
    let spans = pnex_core::directive_spans(&code);
    rsx! {
        if sig.inputs.is_empty() && sig.outputs.is_empty() {
            div { class: "text-sm text-gray-400 italic", {t!("functions-ports-none")} }
        }
        if !sig.inputs.is_empty() {
            InputsSection { code: code.clone(), spans: spans.clone(), sig: sig.clone(), readonly, onedit }
        }
        if !sig.outputs.is_empty() {
            OutputsSection { code: code.clone(), spans, sig, readonly, onedit }
        }
    }
}

/// Inputs grid (name / type / default / label / delete).
#[component]
fn InputsSection(
    code: String,
    spans: Vec<pnex_core::DirectiveSpan>,
    sig: FunctionSignature,
    readonly: bool,
    onedit: EventHandler<String>,
) -> Element {
    let rows: Vec<(u32, pnex_core::FunctionInput)> = sig
        .inputs
        .iter()
        .enumerate()
        .map(|(n, i)| (span_line(&spans, true, n), i.clone()))
        .collect();
    rsx! {
        section { class: "rounded-xl border border-gray-200 bg-white p-3.5 flex flex-col gap-2",
            div { class: "flex items-center gap-2",
                span { class: "w-2.5 h-2.5 rounded-full border-2 border-teal-700" }
                h3 { class: "text-base font-semibold", {t!("functions-ports-in-title")} }
                span { class: "text-xs text-gray-600 bg-gray-100 rounded-full px-2", "{rows.len()}" }
            }
            div { class: "grid grid-cols-[112px_84px_64px_minmax(0,1fr)_28px] gap-1.5 items-center text-xs text-gray-600",
                span { {t!("functions-ports-name")} }
                span { {t!("functions-ports-type")} }
                span { {t!("functions-ports-default")} }
                span { {t!("functions-ports-label")} }
                span {}
            }
            for (line, input) in rows {
                PortRow { code: code.clone(), line, is_input: true, fields: fields_of_input(&input), readonly, onedit }
            }
            if !readonly {
                AddPortButton { code, sig, is_input: true, onedit }
            }
        }
    }
}

/// Outputs grid (name / type / label / delete).
#[component]
fn OutputsSection(
    code: String,
    spans: Vec<pnex_core::DirectiveSpan>,
    sig: FunctionSignature,
    readonly: bool,
    onedit: EventHandler<String>,
) -> Element {
    let rows: Vec<(u32, pnex_core::FunctionOutput)> = sig
        .outputs
        .iter()
        .enumerate()
        .map(|(n, o)| (span_line(&spans, false, n), o.clone()))
        .collect();
    rsx! {
        section { class: "rounded-xl border border-gray-200 bg-white p-3.5 flex flex-col gap-2",
            div { class: "flex items-center gap-2",
                span { class: "w-2.5 h-2.5 rounded-full border-2 border-amber-600" }
                h3 { class: "text-base font-semibold", {t!("functions-ports-out-title")} }
                span { class: "text-xs text-gray-600 bg-gray-100 rounded-full px-2", "{rows.len()}" }
            }
            div { class: "grid grid-cols-[112px_84px_minmax(0,1fr)_28px] gap-1.5 items-center text-xs text-gray-600",
                span { {t!("functions-ports-name")} }
                span { {t!("functions-ports-type")} }
                span { {t!("functions-ports-label")} }
                span {}
            }
            for (line, output) in rows {
                PortRow { code: code.clone(), line, is_input: false, fields: fields_of_output(&output), readonly, onedit }
            }
            if !readonly {
                AddPortButton { code, sig, is_input: false, onedit }
            }
        }
    }
}

/// Directive line (1-based) of the n-th input/output.
fn span_line(spans: &[pnex_core::DirectiveSpan], is_input: bool, n: usize) -> u32 {
    spans
        .iter()
        .filter(|s| s.is_input == is_input)
        .nth(n)
        .map(|s| s.line)
        .unwrap_or(0)
}

/// DirectiveFields from a parsed input/output row.
fn fields_of_input(i: &FunctionInput) -> DirectiveFields {
    DirectiveFields {
        name: i.name.clone(),
        ty: i.ty,
        default: i.default.as_ref().map(default_text),
        desc: i.desc.clone(),
    }
}

fn fields_of_output(o: &FunctionOutput) -> DirectiveFields {
    DirectiveFields {
        name: o.name.clone(),
        ty: o.ty,
        default: None,
        desc: o.desc.clone(),
    }
}

/// Default value rendered as directive text (quoted only when it has spaces).
fn default_text(v: &serde_json::Value) -> String {
    match v {
        serde_json::Value::String(s) => {
            if s.contains(' ') {
                format!("\"{s}\"")
            } else {
                s.clone()
            }
        }
        other => other.to_string(),
    }
}

/// One port row. Cells rewrite the directive line on input.
#[component]
fn PortRow(
    code: String,
    line: u32,
    is_input: bool,
    fields: DirectiveFields,
    readonly: bool,
    onedit: EventHandler<String>,
) -> Element {
    let name_valid = is_valid_port_name(&fields.name);
    let cell = "h-8 rounded-md border border-gray-300 px-2 font-mono text-[13px] min-w-0";
    let cell_bad =
        "h-8 rounded-md border-2 border-red-400 bg-red-50 px-2 font-mono text-[13px] min-w-0";

    // Rewrite the directive line with one field replaced — one closure per
    // cell (each takes its own clone of the current fields).
    macro_rules! rewrite_with {
        ($field:expr, $assign:expr) => {{
            let code = code.clone();
            let fields = fields.clone();
            move |ev: Event<FormData>| {
                let mut f = fields.clone();
                $assign(&mut f, ev.value());
                if let Some(next) = set_directive_fields(&code, line, is_input, &f) {
                    onedit.call(next);
                }
            }
        }};
    }
    let apply_name = rewrite_with!("name", |f: &mut DirectiveFields, v: String| f.name = v);
    let apply_default = rewrite_with!("default", |f: &mut DirectiveFields, v: String| {
        f.default = Some(v);
    });
    let apply_desc = rewrite_with!("desc", |f: &mut DirectiveFields, v: String| {
        f.desc = Some(v);
    });

    rsx! {
        div { class: if is_input {
                "grid grid-cols-[112px_84px_64px_minmax(0,1fr)_28px] gap-1.5 items-center"
            } else {
                "grid grid-cols-[112px_84px_minmax(0,1fr)_28px] gap-1.5 items-center"
            },
            input {
                class: if name_valid { cell } else { cell_bad },
                value: "{fields.name}",
                disabled: readonly,
                oninput: apply_name,
            }
            select {
                class: "{cell}",
                value: "{type_token(fields.ty)}",
                disabled: readonly,
                onchange: {
                    let code = code.clone();
                    let fields = fields.clone();
                    move |ev: Event<FormData>| {
                        let ty = match ev.value().as_str() {
                            "string" => FunctionType::String,
                            "bool" => FunctionType::Bool,
                            "any" => FunctionType::Any,
                            _ => FunctionType::Number,
                        };
                        let mut f = fields.clone();
                        f.ty = ty;
                        if let Some(next) = set_directive_fields(&code, line, is_input, &f) {
                            onedit.call(next);
                        }
                    }
                },
                option { value: "number", "number" }
                option { value: "string", "string" }
                option { value: "bool", "bool" }
                option { value: "any", "any" }
            }
            if is_input {
                input {
                    class: "{cell}",
                    value: fields.default.clone().unwrap_or_default(),
                    disabled: readonly,
                    placeholder: "—",
                    oninput: apply_default,
                }
            } else {
                input { class: "hidden" }
            }
            input {
                class: "{cell} not-mono",
                value: fields.desc.clone().unwrap_or_default(),
                disabled: readonly,
                oninput: apply_desc,
            }
            button {
                class: "text-gray-500 hover:text-red-600 text-lg leading-none",
                disabled: readonly,
                title: t!("functions-ports-delete-title"),
                onclick: {
                    let code = code.clone();
                    move |_| {
                        let next = delete_directive(&code, line);
                        onedit.call(next);
                    }
                },
                "×"
            }
        }
    }
}

/// Dashed add-port button — inserts a new directive after the last one.
#[component]
fn AddPortButton(
    code: String,
    sig: FunctionSignature,
    is_input: bool,
    onedit: EventHandler<String>,
) -> Element {
    let add_label = if is_input {
        t!("functions-ports-add-in")
    } else {
        t!("functions-ports-add-out")
    };
    rsx! {
        button {
            class: "h-9 rounded-lg border border-dashed border-gray-300 bg-white text-sm font-medium text-blue-700 hover:bg-blue-50",
            onclick: move |_| {
                let mut n = 0;
                let base = if is_input { "input" } else { "output" };
                loop {
                    let candidate = format!("{base}_{n}");
                    if !sig.inputs.iter().any(|i| i.name == candidate)
                        && !sig.outputs.iter().any(|o| o.name == candidate)
                    {
                        break;
                    }
                    n += 1;
                }
                let next = insert_directive(&code, is_input, &DirectiveFields {
                    name: format!("{base}_{n}"),
                    ty: FunctionType::Number,
                    default: None,
                    desc: None,
                });
                onedit.call(next);
            },
            {add_label}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rewrite_each_field_and_roundtrip() {
        let code = "// @input temperature number \"Measured temperature\"\n// @output heating number\ndef handle(inputs, msg):\n    return {\"heating\": 1}\n";
        // Rename
        let next = set_directive_fields(
            &code,
            1,
            true,
            &DirectiveFields {
                name: "temp".into(),
                ty: FunctionType::Number,
                default: None,
                desc: None,
            },
        )
        .unwrap();
        assert_eq!(next.lines().nth(0).unwrap(), "// @input temp number");
        assert_eq!(next.lines().nth(1).unwrap(), "// @output heating number");
        // Roundtrip: parse reflects the new fields
        let sig = pnex_core::parse_directives(&next).unwrap();
        assert_eq!(sig.inputs[0].name, "temp");
        // Default + desc
        let next = set_directive_fields(
            &code,
            1,
            true,
            &DirectiveFields {
                name: "temperature".into(),
                ty: FunctionType::Number,
                default: Some("20".into()),
                desc: Some("Measured (°C)".into()),
            },
        )
        .unwrap();
        assert_eq!(
            next.lines().nth(0).unwrap(),
            "// @input temperature number=20 \"Measured (°C)\""
        );
    }
    #[test]
    fn rewrite_respects_line_prefix() {
        let code = "# @input a number\n# @input b string\nx = 1";
        let next = set_directive_fields(
            &code,
            2,
            true,
            &DirectiveFields {
                name: "b".into(),
                ty: FunctionType::String,
                default: Some("auto".into()),
                desc: None,
            },
        )
        .unwrap();
        assert_eq!(next, "# @input a number\n# @input b string=auto\nx = 1");
    }

    #[test]
    fn insert_after_last_directive() {
        let code = "// @input a number\ndef handle(inputs, msg):\n    return 1";
        let next = insert_directive(
            &code,
            false,
            &DirectiveFields {
                name: "out".into(),
                ty: FunctionType::Number,
                default: None,
                desc: None,
            },
        );
        let first = next.lines().next().unwrap();
        let second = next.lines().nth(1).unwrap();
        assert_eq!(first, "// @input a number");
        assert_eq!(second, "// @output out number");
    }

    #[test]
    fn delete_removes_only_target_line() {
        let code = "// @input a number\n// @input b string\nx = 1";
        let next = delete_directive(&code, 1);
        assert_eq!(next, "// @input b string\nx = 1");
    }
}
