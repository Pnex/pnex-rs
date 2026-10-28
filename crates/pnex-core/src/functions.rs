//! Versioned user functions ("Functions" registry, "Automation"
//! group) — types shared backend ↔ frontend ↔ runtime (test
//! CLI), interface directive parser and JS wrapper codegen.
//!
//! Constraints (crate rules): pure serde + `serde_json`, no native
//! dependency — compiles native and `wasm32` (the front calls
//! `parse_directives` in wasm for the live signature preview).
//!
//! Common JS/Starlark execution contract: user code defines
//! `handle` — JS `function handle(inputs, msg)`, Starlark
//! `def handle(inputs, msg):` — `inputs` = object built from the
//! declared inputs (wire routing: `msg.topic` naming an input →
//! incoming payload; otherwise implicit read of `payload.<name>`), `msg` =
//! incoming Node-RED message. Return value (key→value contract):
//! - `null`/`None` → message dropped (Node-RED semantics);
//! - **object + declared `@output` → key→port routing**: each key naming
//!   a declared output emits a msg on that port (payload = the value);
//!   missing or `null` key = silent port;
//! - object without declared outputs → outgoing msg = incoming msg with
//!   `payload` replaced (merged if the object carries `payload`), port 0;
//! - array → one element per port (payload-or-msg rule per element;
//!   `null` element = silent port);
//! - bare value → port 0.
//!
//! Decisions (2026-09-15): JS + Starlark, no Python (sandbox); JS
//! reuses the vendor QuickJS `function` node (generated wrapper), Starlark
//! = custom `pnex-starlark` node; isolating "weird" functions
//! is an infra topic (1 tenant = 1 process), not in-process.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// Language of a registry function: `"js"` or `"starlark"` (stored as
/// text).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "lowercase")]
pub enum FunctionLanguage {
    #[default]
    Js,
    Starlark,
}

impl FunctionLanguage {
    /// Number of output ports of the matching flow node: at least 1
    /// port (Node-RED: a node always has ≥ 1 output), even without `@output`.
    pub fn output_count(outputs: &[FunctionOutput]) -> usize {
        std::cmp::max(1, outputs.len())
    }
}

impl std::fmt::Display for FunctionLanguage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            FunctionLanguage::Js => write!(f, "js"),
            FunctionLanguage::Starlark => write!(f, "starlark"),
        }
    }
}

/// Declared type of an input/output; drives the typed test form (number
/// field, checkbox...) and the documentation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "lowercase")]
pub enum FunctionType {
    Number,
    String,
    Bool,
    Any,
}

/// Input declared by `// @input name type[=default] ["description"]`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct FunctionInput {
    pub name: String,
    pub ty: FunctionType,
    /// Default value when the binding path is absent from the incoming msg.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default: Option<serde_json::Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub desc: Option<String>,
}

/// Output declared by `// @output name type ["description"]`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct FunctionOutput {
    pub name: String,
    pub ty: FunctionType,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub desc: Option<String>,
}

/// Declared interface of a function — extracted from the directives on save,
/// stored structured in `function_versions.inputs` / `.outputs`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct FunctionSignature {
    #[serde(default)]
    pub inputs: Vec<FunctionInput>,
    #[serde(default)]
    pub outputs: Vec<FunctionOutput>,
}

/// Directive parse error (400 on save, line error in the editor).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DirectiveError {
    /// 1-based line in the source code.
    pub line: u32,
    pub message: String,
}

impl std::fmt::Display for DirectiveError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "line {}: {}", self.line, self.message)
    }
}

impl std::error::Error for DirectiveError {}

// ───────────────────────── Directive parser ─────────────────────────

/// Variable name matching `[a-z_][a-z0-9_]*` (valid identifiers in
/// both target languages).
fn is_valid_name(s: &str) -> bool {
    let mut chars = s.chars();
    match chars.next() {
        Some(c) if c == '_' || c.is_ascii_lowercase() => {
            chars.all(|c| c == '_' || c.is_ascii_alphanumeric())
        }
        _ => false,
    }
}

fn parse_type(tok: &str, line: u32) -> Result<FunctionType, DirectiveError> {
    match tok {
        "number" => Ok(FunctionType::Number),
        "string" => Ok(FunctionType::String),
        "bool" => Ok(FunctionType::Bool),
        "any" => Ok(FunctionType::Any),
        other => Err(DirectiveError {
            line,
            message: format!("unknown type \"{other}\" (number|string|bool|any)"),
        }),
    }
}

/// Converts the default text according to the declared type (no spaces in
/// a default: one directive = one line, fields separated by spaces).
fn parse_default(
    ty: FunctionType,
    text: &str,
    line: u32,
) -> Result<serde_json::Value, DirectiveError> {
    let bad = |message: String| DirectiveError { line, message };
    match ty {
        FunctionType::Number => text
            .parse::<f64>()
            .map(|n| serde_json::json!(n))
            .map_err(|_| bad(format!("default \"{text}\": number expected"))),
        FunctionType::String => Ok(serde_json::Value::String(
            text.trim_matches('"').to_string(),
        )),
        FunctionType::Bool => match text {
            "true" => Ok(serde_json::Value::Bool(true)),
            "false" => Ok(serde_json::Value::Bool(false)),
            _ => Err(bad(format!(
                "default \"{text}\": bool expected (true|false)"
            ))),
        },
        FunctionType::Any => serde_json::from_str(text)
            .map_err(|e| bad(format!("default \"{text}\": invalid JSON ({e})"))),
    }
}

/// Strips the enclosing quotes of an optional description.
fn clean_desc(s: &str) -> Option<String> {
    let s = s.trim();
    if s.is_empty() {
        return None;
    }
    let unquoted = if s.len() >= 2 && s.starts_with('"') && s.ends_with('"') {
        &s[1..s.len() - 1]
    } else {
        s
    };
    if unquoted.is_empty() {
        None
    } else {
        Some(unquoted.to_string())
    }
}

/// Extracts the signature declared by the `@input`/`@output` directives in
/// comments (`//` or `#`, tolerant for both languages). Lines
/// that are not directives are ignored; typed errors with the
/// offending line (400 on save, preview in the front).
pub fn parse_directives(code: &str) -> Result<FunctionSignature, DirectiveError> {
    let mut sig = FunctionSignature::default();
    let mut seen_inputs: std::collections::BTreeSet<String> = Default::default();
    let mut seen_outputs: std::collections::BTreeSet<String> = Default::default();

    for (idx, raw) in code.lines().enumerate() {
        let line_no = (idx as u32) + 1;
        let trimmed = raw.trim();
        let Some(body) = trimmed
            .strip_prefix("//")
            .or_else(|| trimmed.strip_prefix('#'))
            .map(str::trim_start)
        else {
            continue;
        };
        if let Some(rest) = body.strip_prefix("@input") {
            // Strict boundary: `@inputs`/`@input_x` are not
            // directives (the word must be followed by whitespace or end of line).
            if !rest.is_empty() && !rest.starts_with(' ') && !rest.starts_with('\t') {
                continue;
            }
            let rest = rest.trim_start();
            let mut words = rest.splitn(2, char::is_whitespace);
            let name = words.next().unwrap_or("");
            let rest2 = words.next().unwrap_or("").trim();
            if !is_valid_name(name) {
                return Err(DirectiveError {
                    line: line_no,
                    message: format!(
                        "@input \"{name}\": snake_case name expected ([a-z_][a-z0-9_]*)"
                    ),
                });
            }
            let mut words2 = rest2.splitn(2, char::is_whitespace);
            let ty_word = words2.next().unwrap_or("");
            let desc_part = words2.next().unwrap_or("");
            let (ty_tok, dflt_text) = match ty_word.split_once('=') {
                Some((t, d)) => (t, Some(d)),
                None => (ty_word, None),
            };
            let ty = if ty_tok.is_empty() {
                return Err(DirectiveError {
                    line: line_no,
                    message: format!("@input {name}: type required (number|string|bool|any)"),
                });
            } else {
                parse_type(ty_tok, line_no)?
            };
            let default = match dflt_text {
                Some(text) => Some(parse_default(ty, text, line_no)?),
                None => None,
            };
            if !seen_inputs.insert(name.to_string()) {
                return Err(DirectiveError {
                    line: line_no,
                    message: format!("@input {name}: duplicate input"),
                });
            }
            sig.inputs.push(FunctionInput {
                name: name.to_string(),
                ty,
                default,
                desc: clean_desc(desc_part),
            });
        } else if let Some(rest) = body.strip_prefix("@output") {
            if !rest.is_empty() && !rest.starts_with(' ') && !rest.starts_with('\t') {
                continue;
            }
            let rest = rest.trim_start();
            let mut words = rest.splitn(2, char::is_whitespace);
            let name = words.next().unwrap_or("");
            let rest2 = words.next().unwrap_or("").trim();
            if !is_valid_name(name) {
                return Err(DirectiveError {
                    line: line_no,
                    message: format!(
                        "@output \"{name}\": snake_case name expected ([a-z_][a-z0-9_]*)"
                    ),
                });
            }
            let mut words2 = rest2.splitn(2, char::is_whitespace);
            let ty_word = words2.next().unwrap_or("");
            let desc_part = words2.next().unwrap_or("");
            let ty = parse_type(ty_word, line_no)?;
            if !seen_outputs.insert(name.to_string()) {
                return Err(DirectiveError {
                    line: line_no,
                    message: format!("@output {name}: duplicate output"),
                });
            }
            sig.outputs.push(FunctionOutput {
                name: name.to_string(),
                ty,
                desc: clean_desc(desc_part),
            });
        }
        // Any other comment: ignored.
    }
    Ok(sig)
}

/// Position of an `@input`/`@output` directive in the code (1-based line).
/// Used by the editable port grids: the line identifies the directive to
/// rewrite when the user edits a cell.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DirectiveSpan {
    pub is_input: bool,
    pub name: String,
    pub line: u32,
}

/// Positions of all directives, tolerant to errors and
/// duplicates (unlike [`parse_directives`], which fails): the editor
/// needs to locate the directives even in invalid code.
pub fn directive_spans(code: &str) -> Vec<DirectiveSpan> {
    let mut spans = Vec::new();
    for (idx, raw) in code.lines().enumerate() {
        let trimmed = raw.trim();
        let Some(body) = trimmed
            .strip_prefix("//")
            .or_else(|| trimmed.strip_prefix('#'))
            .map(str::trim_start)
        else {
            continue;
        };
        for (word, is_input) in [("@input", true), ("@output", false)] {
            if let Some(rest) = body.strip_prefix(word) {
                // Same strict boundary as `parse_directives`: the word
                // must be followed by whitespace or end of line.
                if !rest.is_empty() && !rest.starts_with(' ') && !rest.starts_with('\t') {
                    continue;
                }
                if let Some(name) = rest.split_whitespace().next() {
                    spans.push(DirectiveSpan {
                        is_input,
                        name: name.to_string(),
                        line: (idx as u32) + 1,
                    });
                }
                break;
            }
        }
    }
    spans
}

// ───────────────────────── Bindings & chemins msg ─────────────────────────

/// Default binding of an input: `payload.<name>`.
pub fn default_binding(input_name: &str) -> String {
    format!("payload.{input_name}")
}

/// Resolves a dotted path (`payload.temperature`, numeric segments =
/// array index) in a JSON msg. `None` = path missing.
pub fn resolve_msg_path<'a>(
    msg: &'a serde_json::Value,
    path: &str,
) -> Option<&'a serde_json::Value> {
    let mut cur = msg;
    for seg in path.split('.') {
        match cur {
            serde_json::Value::Object(map) => cur = map.get(seg)?,
            serde_json::Value::Array(items) => {
                let idx: usize = seg.parse().ok()?;
                cur = items.get(idx)?;
            }
            _ => return None,
        }
    }
    Some(cur)
}

/// Inserts a value at a dotted path of the msg (creates intermediate
/// objects; overwrites an existing value). Mirror of
/// [`resolve_msg_path`] for the test panel: the values of the typed
/// form are injected at the binding paths before execution.
pub fn insert_msg_path(msg: &mut serde_json::Value, path: &str, value: serde_json::Value) {
    let segments: Vec<&str> = path.split('.').collect();
    if segments.is_empty() {
        return;
    }
    let mut cur = msg;
    for (i, seg) in segments.iter().enumerate() {
        if i == segments.len() - 1 {
            if !cur.is_object() {
                *cur = serde_json::Value::Object(serde_json::Map::new());
            }
            cur.as_object_mut()
                .expect("object ensured")
                .insert((*seg).to_string(), value);
            return;
        }
        if !cur.is_object() {
            *cur = serde_json::Value::Object(serde_json::Map::new());
        }
        cur = cur
            .as_object_mut()
            .expect("object ensured")
            .entry((*seg).to_string())
            .or_insert(serde_json::Value::Object(serde_json::Map::new()));
    }
}

/// Builds the `inputs` object passed to `handle`. Wire routing: if
/// `msg.topic` names a declared input (wire set on its anchor — tagger
/// inserted by the projection, or JSON Split output), the input receives the
/// **incoming payload**; otherwise it implicitly reads `payload.<name>`;
/// missing → declared default; otherwise `null` (documented in the cheat-sheet).
pub fn build_inputs(
    inputs: &[FunctionInput],
    msg: &serde_json::Value,
) -> serde_json::Map<String, serde_json::Value> {
    let topic = msg.get("topic").and_then(serde_json::Value::as_str);
    inputs
        .iter()
        .map(|input| {
            let value = if topic == Some(input.name.as_str()) {
                msg.get("payload")
                    .cloned()
                    .unwrap_or(serde_json::Value::Null)
            } else {
                let path = default_binding(&input.name);
                resolve_msg_path(msg, &path)
                    .cloned()
                    .or_else(|| input.default.clone())
                    .unwrap_or(serde_json::Value::Null)
            };
            (input.name.clone(), value)
        })
        .collect()
}

// ───────────────────────── Codegen wrapper JS ─────────────────────────

/// Generates the body of `async function __el_user_func(msg) { … }` consumed
/// by the `function` node of the EdgeLinkd vendor (QuickJS): embedded helpers
/// + user code (defining `handle`) + tail that builds `inputs`
///   (each input implicitly reads `payload.<name>`, declared defaults
///   included) and maps the return value to ports — **key→value object routed by
///   declared `@output`** (missing key = silent port) when outputs are
///   declared, historical contract otherwise. Strings are inserted as
///   escaped JS literals (serde_json).
pub fn build_js_wrapper(
    user_code: &str,
    inputs: &[FunctionInput],
    outputs: &[FunctionOutput],
) -> String {
    const HELPERS: &str = r#"// ── PNEX function wrapper (generated) ──
function __pnex_get(msg, path, dflt) {
  let cur = msg;
  for (const seg of String(path).split(".")) {
    if (cur === null || typeof cur !== "object") return dflt;
    cur = cur[seg];
  }
  return cur === undefined || cur === null ? dflt : cur;
}
function __pnex_out(msg, r) {
  if (r === null || r === undefined) return null;
  if (typeof r === "object" && "payload" in r) return Object.assign({}, msg, r);
  return Object.assign({}, msg, { payload: r });
}
function __pnex_port(msg, v) {
  if (v === null || v === undefined) return null;
  return Object.assign({}, msg, { payload: v });
}
"#;

    // Tail generated from the declared outputs: **key→value object routed
    // per port** (missing/null key = silent port); otherwise historical contract
    // (positional array, value/object → port 0).
    let tail = if outputs.is_empty() {
        "const __pnex_r = await Promise.resolve(handle(__pnex_inputs, msg));\n\
         if (__pnex_r === null || __pnex_r === undefined) return null;\n\
         if (Array.isArray(__pnex_r)) return __pnex_r.map(function (x) { return __pnex_out(msg, x); });\n\
         return [__pnex_out(msg, __pnex_r)];\n"
            .to_string()
    } else {
        let ports = outputs
            .iter()
            .map(|o| {
                let lit = serde_json::to_string(&o.name).unwrap_or_default();
                format!("__pnex_port(msg, __pnex_r[{lit}])")
            })
            .collect::<Vec<_>>()
            .join(", ");
        format!(
            "const __pnex_r = await Promise.resolve(handle(__pnex_inputs, msg));\n\
             if (__pnex_r === null || __pnex_r === undefined) return null;\n\
             if (typeof __pnex_r === \"object\" && !Array.isArray(__pnex_r)) return [{ports}];\n\
             if (Array.isArray(__pnex_r)) return __pnex_r.map(function (x) {{ return __pnex_out(msg, x); }});\n\
             return [__pnex_out(msg, __pnex_r)];\n"
        )
    };

    let mut js = String::with_capacity(user_code.len() + 1024);
    js.push_str(HELPERS);
    js.push('\n');
    js.push_str(user_code.trim_end());
    js.push_str("\n\nconst __pnex_inputs = {\n");
    for input in inputs {
        let path = default_binding(&input.name);
        let name_lit = serde_json::to_string(&input.name).unwrap_or_default();
        let path_lit = serde_json::to_string(&path).unwrap_or_default();
        let dflt_lit = input
            .default
            .as_ref()
            .map(|v| serde_json::to_string(v).unwrap_or_else(|_| "null".into()))
            .unwrap_or_else(|| "undefined".to_string());
        // Wire routing: `topic` naming the input → incoming payload.
        js.push_str(&format!(
            "  {name_lit}: (typeof msg.topic === \"string\" && msg.topic === {name_lit}) ? (msg.payload === undefined ? null : msg.payload) : __pnex_get(msg, {path_lit}, {dflt_lit}),\n"
        ));
    }
    js.push_str("};\n");
    js.push_str(&tail);
    js
}

/// Generates the body of the **tagger** inserted by the projection on each wire
/// arriving on the anchor of a declared function input: stamps
/// `msg.topic` with the input name — both the JS wrapper and the Starlark
/// executor then route the incoming payload to `inputs.<name>` (see
/// `build_inputs` / `build_js_wrapper`). The text is the **body** of
/// `__el_user_func(msg)` (vendor `function` node contract): the `return`
/// must be at body level — a `function handle() {}` defined without
/// being called returns nothing and the message would be dropped.
pub fn build_input_tag_func(pin: &str) -> String {
    let pin_lit = serde_json::to_string(pin).unwrap_or_else(|_| "\"\"".into());
    format!("return Object.assign({{}}, msg, {{ topic: {pin_lit} }});")
}

// ───────────────── JSON Split / JSON Merge generators ─────────────────

/// Generates the vendor `function` node body for a `json-split` node.
/// Empty `keys` (legacy): object → one msg per key on the single port
/// (`payload` = value, `topic` = key), array → one msg per element
/// (`topic` = index); any other payload passes unchanged. Non-empty `keys`:
/// one port **named per key** — `payload[key]` goes out on its port, missing key
/// = silent port (`null` slot, vendor contract), array → elements on port
/// 0, other payload → passthrough port 0; key without a port = ignored.
pub fn build_json_split_func(keys: &[String]) -> String {
    if keys.is_empty() {
        return r#"const p = msg.payload;
if (Array.isArray(p)) {
  return [p.map(function (v, i) {
    return Object.assign({}, msg, { payload: v, topic: String(i) });
  })];
}
if (p !== null && typeof p === "object") {
  return [Object.keys(p).map(function (k) {
    return Object.assign({}, msg, { payload: p[k], topic: k });
  })];
}
return [msg];"#
            .to_string();
    }
    let keys_lit = serde_json::to_string(keys).unwrap_or_else(|_| "[]".into());
    format!(
        r#"var keys = {keys_lit};
var p = msg.payload;
if (Array.isArray(p)) {{
  return [p.map(function (v, i) {{
    return Object.assign({{}}, msg, {{ payload: v, topic: String(i) }});
  }})];
}}
if (p !== null && typeof p === "object") {{
  var ports = [];
  for (var i = 0; i < keys.length; i++) {{
    var k = keys[i];
    if (Object.prototype.hasOwnProperty.call(p, k)) {{
      ports.push([Object.assign({{}}, msg, {{ payload: p[k], topic: k }})]);
    }} else {{
      ports.push(null);
    }}
  }}
  return ports;
}}
return [msg];"#
    )
}

/// Generates the vendor `function` node body for a `json-merge` node:
/// each payload is stored under `msg.topic` (if non-empty), otherwise under
/// `default_key` in a persistent accumulator (node context) — one
/// input = one variable; the accumulated object is emitted on every msg. Deliberately
/// `var`/`Object.assign` style (no spread/arrow) — same school as the
/// function wrapper.
pub fn build_json_merge_func(default_key: &str) -> String {
    let key_lit = serde_json::to_string(default_key).unwrap_or_else(|_| "\"value\"".into());
    format!(
        r#"var key = (typeof msg.topic === "string" && msg.topic) ? msg.topic : {key_lit};
var acc = context.get("__pnex_merge");
if (acc === null || typeof acc !== "object" || Array.isArray(acc)) acc = {{}};
acc = Object.assign({{}}, acc);
acc[key] = msg.payload;
context.set("__pnex_merge", acc);
return [Object.assign({{}}, msg, {{ payload: acc }})];"#
    )
}

// ───────────────────── Flow node config + snapshots ─────────────────────

/// Configuration of the custom PNEX `PnexFunction` node — **references +
/// interface snapshot** only: the CODE is never in the graph,
/// the projection inlines it at deploy from the pinned version (self-contained
/// artifact; editing a function does not affect deployed flows
/// until they are redeployed).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct FunctionNodeConfig {
    /// `functions.id` — 0 = not selected (validate_graph: `fn_not_selected`).
    #[serde(default)]
    pub function_id: i64,
    /// Name kept for display (the picker fills it).
    #[serde(default)]
    pub function_name: String,
    /// Pinned `function_versions.version_number` — 0 = not set.
    #[serde(default)]
    pub version_number: i64,
    #[serde(default)]
    pub language: FunctionLanguage,
    /// Snapshot of the declared inputs/outputs (directives of the pinned
    /// version) — drives the inspector and the test panel.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub inputs: Vec<FunctionInput>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub outputs: Vec<FunctionOutput>,
}

/// Snapshot of a function version resolved at deploy (backend) —
/// carries the CODE that will be inlined in the artifact.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FunctionSnapshot {
    pub function_id: i64,
    pub version_number: i64,
    pub language: FunctionLanguage,
    pub code: String,
    #[serde(default)]
    pub inputs: Vec<FunctionInput>,
    #[serde(default)]
    pub outputs: Vec<FunctionOutput>,
}

/// Resolver passed to the projection: `(function_id, version_number) →
/// snapshot`. The backend builds it in batch from `function_versions`.
pub type FunctionResolver = BTreeMap<(i64, i64), FunctionSnapshot>;

// ───────────────────────────── API DTOs ─────────────────────────────

/// Function summary (paginated list, without the code).
/// `PartialEq` is required by the CRUD component props (dioxus 0.7
/// `#[component]` derives a per-field impl).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct FunctionSummary {
    pub id: i64,
    pub org_id: i64,
    pub name: String,
    pub language: FunctionLanguage,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub current_version_number: i64,
    #[cfg_attr(feature = "openapi", schema(format = DateTime))]
    pub created_at: String,
    #[cfg_attr(feature = "openapi", schema(format = DateTime))]
    pub updated_at: String,
}

/// A function with its current version (code and interface).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct FunctionDetail {
    pub id: i64,
    pub org_id: i64,
    pub name: String,
    pub language: FunctionLanguage,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub current_version_number: i64,
    pub code: String,
    #[serde(default)]
    pub inputs: Vec<FunctionInput>,
    #[serde(default)]
    pub outputs: Vec<FunctionOutput>,
    #[cfg_attr(feature = "openapi", schema(format = DateTime))]
    pub created_at: String,
    #[cfg_attr(feature = "openapi", schema(format = DateTime))]
    pub updated_at: String,
}

/// One version of a function (history drawer, and the version picker of
/// the flow inspector, which takes the interface snapshot from here).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct FunctionVersionSummary {
    pub id: i64,
    pub version_number: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub inputs: Vec<FunctionInput>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub outputs: Vec<FunctionOutput>,
    #[cfg_attr(feature = "openapi", schema(format = DateTime))]
    pub created_at: String,
    /// Full code of this version — the history drawer restores ANY past
    /// version into the editor (append-only: saving then creates v+1).
    pub code: String,
}

/// Creation: the function and its version 1, in one transaction.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct CreateFunction {
    pub name: String,
    pub language: FunctionLanguage,
    #[serde(default)]
    pub description: Option<String>,
    pub code: String,
    #[serde(default)]
    pub note: Option<String>,
}

/// Saves a new version (append-only). Optimistic concurrency:
/// `expected_version_number` must be the current version, else 409.
/// `name`/`description` update the metadata; with `code`, the save
/// creates a new version.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct SaveFunctionVersion {
    pub expected_version_number: i64,
    #[serde(default)]
    pub code: Option<String>,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub note: Option<String>,
}

/// Ad-hoc test of unsaved code (test before saving).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct FunctionTestAdHoc {
    pub language: FunctionLanguage,
    pub code: String,
}

/// Live test request: a pinned version of the function (default: the
/// current version) OR ad-hoc code. `inputs` = typed form values; `msg` =
/// simulated incoming Node-RED message (default `{}`).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct FunctionTestRequest {
    #[serde(default)]
    pub version_number: Option<i64>,
    #[serde(default)]
    pub ad_hoc: Option<FunctionTestAdHoc>,
    #[serde(default)]
    pub inputs: BTreeMap<String, serde_json::Value>,
    #[serde(default)]
    pub msg: Option<serde_json::Value>,
}

/// Live test answer (also the single output line of the
/// `pnex-flow-runtime --test-function` CLI).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct FunctionTestResponse {
    pub ok: bool,
    /// Outgoing messages (one per port that produced a message, in order).
    pub outputs: Vec<serde_json::Value>,
    /// Captured `console.*` output (test mode).
    pub logs: Vec<String>,
    pub duration_ms: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// Request given to the `pnex-flow-runtime --test-function` CLI. The JS
/// wrapper is already generated by the backend (`build_js_wrapper`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FunctionExecRequest {
    pub language: FunctionLanguage,
    /// Full JS wrapper (helpers + code + tail), required for `js`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub js_wrapper: Option<String>,
    /// Raw Starlark code, required for `starlark`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub code: Option<String>,
    #[serde(default)]
    pub inputs: Vec<FunctionInput>,
    pub msg: serde_json::Value,
    pub timeout_ms: u64,
}

/// Compile-only validation request (editor error bar) of
/// `POST /api/v1/functions/validate`, passed as is to the
/// `pnex-flow-runtime --check-function` CLI. The code is never executed.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct FunctionValidateRequest {
    pub language: FunctionLanguage,
    pub code: String,
}

/// Editor diagnostic: 1-based line/column, best effort (`null` when the
/// engine does not expose them). The message is the engine's, verbatim.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct FunctionDiagnostic {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub line: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub col: Option<u32>,
    pub message: String,
}

/// Compile-only validation answer (also the single output line of the
/// `pnex-flow-runtime --check-function` CLI).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct FunctionValidateResponse {
    pub ok: bool,
    pub diagnostics: Vec<FunctionDiagnostic>,
}

// ──────────────────────────────── Tests ────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parse_directives_complet() {
        let code = r#"
// @input temperature number "Température mesurée"  english-guard: allow (French example)
// @input mode string="auto" Mode de marche
// @input enabled bool=true
# @output heating number
# @output alarm bool "Alarme process"
function handle(inputs, msg) { return inputs.temperature; }
"#;
        let sig = parse_directives(code).unwrap();
        assert_eq!(sig.inputs.len(), 3);
        assert_eq!(sig.outputs.len(), 2);
        assert_eq!(sig.inputs[0].name, "temperature");
        assert_eq!(sig.inputs[0].ty, FunctionType::Number);
        assert_eq!(sig.inputs[0].desc.as_deref(), Some("Température mesurée"));
        assert_eq!(sig.inputs[1].ty, FunctionType::String);
        assert_eq!(sig.inputs[1].default, Some(json!("auto")));
        assert_eq!(sig.inputs[1].desc.as_deref(), Some("Mode de marche"));
        assert_eq!(sig.inputs[2].default, Some(json!(true)));
        assert_eq!(sig.outputs[0].name, "heating");
        assert_eq!(sig.outputs[1].desc.as_deref(), Some("Alarme process"));
    }

    #[test]
    fn parse_directives_vide_et_sans_directive() {
        assert!(parse_directives("").unwrap().inputs.is_empty());
        assert!(parse_directives("// nothing here\nlet x = 1;")
            .unwrap()
            .outputs
            .is_empty());
    }

    #[test]
    fn parse_directives_erreurs() {
        // type inconnu
        let e = parse_directives("// @input a float").unwrap_err();
        assert_eq!(e.line, 1);
        assert!(e.message.contains("unknown type"), "{}", e.message);
        // non-convertible default
        let e = parse_directives("// @input a number=abc").unwrap_err();
        assert!(e.message.contains("number expected"), "{}", e.message);
        // duplicated
        let e = parse_directives("// @input a number\n// @input a string").unwrap_err();
        assert!(e.message.contains("duplicate"), "{}", e.message);
        // invalid name
        let e = parse_directives("# @input 1abc number").unwrap_err();
        assert!(e.message.contains("snake_case"), "{}", e.message);
        // type manquant
        let e = parse_directives("// @input a").unwrap_err();
        assert!(e.message.contains("type required"), "{}", e.message);
    }

    #[test]
    fn parse_directives_defaut_any_json() {
        let sig = parse_directives("// @input cfg any={\"limit\":5}").unwrap();
        assert_eq!(sig.inputs[0].default, Some(json!({"limit": 5})));
    }

    #[test]
    fn directive_spans_positions() {
        let code = "def handle(inputs, msg):\n\
                    # @input temperature number\n\
                    return inputs[\"temperature\"]\n\
                    // @output heating number\n\
                    // @output delta any\n\
                    // not a directive\n\
                    # @inputs nope";
        let spans = directive_spans(code);
        assert_eq!(
            spans,
            vec![
                DirectiveSpan {
                    is_input: true,
                    name: "temperature".into(),
                    line: 2
                },
                DirectiveSpan {
                    is_input: false,
                    name: "heating".into(),
                    line: 4
                },
                DirectiveSpan {
                    is_input: false,
                    name: "delta".into(),
                    line: 5
                },
            ]
        );
    }

    #[test]
    fn directive_spans_tolerant() {
        // Invalid directives still yield spans (the editor needs positions
        // even on broken code); `@inputs` strict boundary is skipped.
        let spans = directive_spans("// @input temp\n// @input\n// @inputs x number");
        assert_eq!(spans.len(), 1);
        assert_eq!(spans[0].line, 1);
        assert_eq!(spans[0].name, "temp");
        assert!(directive_spans("").is_empty());
        assert!(directive_spans("x = 1").is_empty());
    }

    #[test]
    fn resolve_msg_path_chemins() {
        let msg = json!({"payload": {"t": [10, 20]}, "topic": "a"});
        assert_eq!(resolve_msg_path(&msg, "payload.t.1"), Some(&json!(20)));
        assert_eq!(resolve_msg_path(&msg, "topic"), Some(&json!("a")));
        assert_eq!(resolve_msg_path(&msg, "payload.t.9"), None);
        assert_eq!(resolve_msg_path(&msg, "payload.x.y"), None);
    }

    #[test]
    fn insert_msg_path_cree_les_niveaux() {
        let mut msg = serde_json::json!({"topic": "a"});
        insert_msg_path(&mut msg, "payload.temperature", serde_json::json!(10));
        assert_eq!(msg["payload"]["temperature"], 10);
        assert_eq!(msg["topic"], "a");
        // Overwrites an existing value.
        insert_msg_path(&mut msg, "payload.temperature", serde_json::json!(20));
        assert_eq!(msg["payload"]["temperature"], 20);
        // Root path traversing a non-object: replaced by an object.
        let mut scalar = serde_json::json!(42);
        insert_msg_path(&mut scalar, "payload.x", serde_json::json!(1));
        assert_eq!(scalar["payload"]["x"], 1);
        // Roundtrip with resolve_msg_path.
        assert_eq!(
            resolve_msg_path(&msg, "payload.temperature"),
            Some(&serde_json::json!(20))
        );
    }

    #[test]
    fn build_inputs_default_payload_name() {
        let inputs = vec![
            FunctionInput {
                name: "t".into(),
                ty: FunctionType::Number,
                default: Some(json!(0)),
                desc: None,
            },
            FunctionInput {
                name: "mode".into(),
                ty: FunctionType::String,
                default: None,
                desc: None,
            },
        ];
        // Each input implicitly reads payload.<name>: t ← payload.t
        // (missing → declared default 0); mode ← payload.mode (missing, no
        // default → null).
        let inputs_obj = build_inputs(&inputs, &json!({}));
        assert_eq!(inputs_obj.get("t"), Some(&json!(0)));
        assert_eq!(
            inputs_obj.get("mode"),
            Some(&json!(serde_json::Value::Null))
        );
        let inputs_obj = build_inputs(&inputs, &json!({"payload": {"t": 42}}));
        assert_eq!(inputs_obj.get("t"), Some(&json!(42)));
    }

    #[test]
    fn build_inputs_topic_route_le_payload() {
        let inputs = vec![FunctionInput {
            name: "t".into(),
            ty: FunctionType::Number,
            default: Some(json!(0)),
            desc: None,
        }];
        // msg.topic naming the input → incoming payload (wire routing),
        // even if payload.<name> exists as well.
        let inputs_obj = build_inputs(&inputs, &json!({"topic": "t", "payload": 42}));
        assert_eq!(inputs_obj.get("t"), Some(&json!(42)));
        // Unknown topic → implicit read of payload.<name> (fallback).
        let inputs_obj = build_inputs(&inputs, &json!({"topic": "autre", "payload": {"t": 7}}));
        assert_eq!(inputs_obj.get("t"), Some(&json!(7)));
        // Topic naming the input but payload missing → null.
        let inputs_obj = build_inputs(&inputs, &json!({"topic": "t"}));
        assert_eq!(inputs_obj.get("t"), Some(&json!(serde_json::Value::Null)));
    }

    #[test]
    fn build_input_tag_func_estampe_le_topic() {
        let js = build_input_tag_func("in_float");
        // Direct body of __el_user_func: the return at body level
        // (a function handle defined without being called → message dropped).
        assert!(
            js.starts_with("return Object.assign"),
            "corps direct, return top-level"
        );
        assert!(
            js.contains("topic: \"in_float\""),
            "the tagger stamps topic with the input name"
        );
        assert!(
            !js.contains("function handle"),
            "never an uncalled handle declaration"
        );
    }

    #[test]
    fn build_js_wrapper_structure() {
        let inputs = vec![FunctionInput {
            name: "t".into(),
            ty: FunctionType::Number,
            default: Some(json!(21.5)),
            desc: None,
        }];
        let code = "function handle(inputs, msg) { return inputs.t + 1; }";
        let js = build_js_wrapper(code, &inputs, &[]);
        assert!(js.contains("__pnex_get"), "helpers absents");
        assert!(js.contains("function handle(inputs, msg)"), "code user");
        assert!(js.contains("\"payload.t\""), "implicit payload.<name> path");
        assert!(js.contains("21.5"), "serialized default");
        assert!(js.contains("__pnex_inputs"), "objet inputs");
        assert!(js.contains("msg.topic === \"t\""), "topic routing branch");
        assert!(js.contains("await Promise.resolve(handle("), "tail");
        // Escaping: an input name containing a quote does not break the JS.
        let evil = vec![FunctionInput {
            name: "a\"b".into(),
            ty: FunctionType::Number,
            default: None,
            desc: None,
        }];
        let js = build_js_wrapper(code, &evil, &[]);
        assert!(
            js.contains("\"payload.a\\\"b\""),
            "JSON escaping of the path"
        );
    }

    #[test]
    fn build_js_wrapper_route_le_retour_par_sorties() {
        let fout = |name: &str| FunctionOutput {
            name: name.into(),
            ty: FunctionType::Bool,
            desc: None,
        };
        let code = "function handle(inputs, msg) { return {out_bool: true}; }";
        let js = build_js_wrapper(code, &[], &[fout("out_bool"), fout("out_temp")]);
        // Key→value object routed per port, in @output order.
        assert!(
            js.contains(
                "return [__pnex_port(msg, __pnex_r[\"out_bool\"]), __pnex_port(msg, __pnex_r[\"out_temp\"])];"
            ),
            "generated key→port routing"
        );
        // Without declared outputs: historical contract (port 0).
        let js = build_js_wrapper(code, &[], &[]);
        assert!(
            !js.contains("__pnex_port(msg, __pnex_r["),
            "no routing without @output"
        );
        assert!(
            js.contains("return [__pnex_out(msg, __pnex_r)];"),
            "historical return port 0"
        );
    }

    #[test]
    fn json_split_genere_fan_out_par_cle() {
        let js = build_json_split_func(&[]);
        assert!(
            js.contains("Array.isArray(p)"),
            "array branch (fan-out per element)"
        );
        assert!(
            js.contains("Object.keys(p).map"),
            "fan-out per key (object)"
        );
        assert!(js.contains("topic: k"), "topic = key");
        assert!(
            js.contains("return [msg]"),
            "pass-through of payloads neither object nor array"
        );
        // No key literal: legacy 1-port contract kept verbatim.
        assert!(!js.contains("var keys"), "legacy without declared keys");
    }

    #[test]
    fn json_split_genere_ports_nommes() {
        let js = build_json_split_func(&["a".to_string(), "b".to_string()]);
        assert!(
            js.contains(r#"var keys = ["a","b"]"#),
            "inline key literal (serde_json): {js}"
        );
        assert!(
            js.contains("hasOwnProperty.call(p, k)"),
            "slot only for present keys"
        );
        assert!(
            js.contains("ports.push(null)"),
            "silent port (null slot) for a missing key"
        );
        assert!(
            js.contains("Array.isArray(p)"),
            "array → port 0 (all elements)"
        );
        assert!(
            js.contains("return [msg]"),
            "passthrough port 0 of the other payloads"
        );
    }

    #[test]
    fn json_merge_genere_accumulation_contexte() {
        let js = build_json_merge_func("ma clé");
        assert!(
            js.contains("context.get(\"__pnex_merge\")"),
            "lecture accumulateur"
        );
        assert!(
            js.contains("context.set(\"__pnex_merge\", acc)"),
            "accumulator write"
        );
        assert!(
            js.contains("acc[key] = msg.payload"),
            "one input = one variable (no more object shallow-merge)"
        );
        assert!(
            !js.contains("Object.assign({}, acc, msg.payload)"),
            "object shallow-merge branch removed"
        );
        assert!(
            js.contains("\"ma clé\""),
            "default key inlined and escaped (serde_json)"
        );
    }

    #[test]
    fn language_output_count_min_1() {
        assert_eq!(FunctionLanguage::output_count(&[]), 1);
    }

    #[test]
    fn dto_roundtrip() {
        let detail = FunctionDetail {
            id: 3,
            org_id: 7,
            name: "chauffage".into(),
            language: FunctionLanguage::Starlark,
            description: Some("d".into()),
            current_version_number: 2,
            code: "def handle(inputs, msg):".into(),
            inputs: vec![],
            outputs: vec![],
            created_at: "2026-09-15T00:00:00Z".into(),
            updated_at: "2026-09-15T00:00:00Z".into(),
        };
        let back: FunctionDetail =
            serde_json::from_str(&serde_json::to_string(&detail).unwrap()).unwrap();
        assert_eq!(back.language, FunctionLanguage::Starlark);
        assert_eq!(back.current_version_number, 2);
    }
}
