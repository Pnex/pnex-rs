//! Versioned user functions ("Functions" registry, "Automation"
//! group) — types shared backend ↔ frontend ↔ runtime (test
//! CLI), signature validation and JS wrapper codegen.
//!
//! Constraints (crate rules): pure serde + `serde_json`, no native
//! dependency — compiles native and `wasm32`.
//!
//! The interface (`FunctionSignature`) is structured data of the version,
//! never text in the code (D497).
//!
//! Common JS/Starlark execution contract: user code defines
//! `handle` — JS `function handle(inputs, msg)`, Starlark
//! `def handle(inputs, msg):` — `inputs` = object built from the
//! declared inputs (wire routing: `msg.topic` naming an input →
//! incoming payload; otherwise implicit read of `payload.<name>`), `msg` =
//! incoming Node-RED message. Return value (key→value contract):
//! - `null`/`None` → message dropped (Node-RED semantics);
//! - **object + declared outputs → key→port routing**: each key naming
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
    /// port (Node-RED: a node always has ≥ 1 output), even without a declared output.
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

/// Declared input: one input anchor of the flow node.
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

/// Declared output: one output port of the flow node.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct FunctionOutput {
    pub name: String,
    pub ty: FunctionType,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub desc: Option<String>,
}

/// Declared interface of a function version (D497), stored in
/// `function_versions.inputs` / `.outputs`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct FunctionSignature {
    #[serde(default)]
    pub inputs: Vec<FunctionInput>,
    #[serde(default)]
    pub outputs: Vec<FunctionOutput>,
}

/// Invalid signature field: `field` = path (`inputs.0.name`), `code` =
/// machine token (`invalid`, `unique`, `required`) — a 400 field error.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SignatureError {
    pub field: String,
    pub code: String,
}

impl std::fmt::Display for SignatureError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.field, self.code)
    }
}

impl std::error::Error for SignatureError {}

/// Port name matching `[a-z_][a-z0-9_]*` (valid identifiers in both
/// target languages).
pub fn is_valid_port_name(s: &str) -> bool {
    let mut chars = s.chars();
    match chars.next() {
        Some(c) if c == '_' || c.is_ascii_lowercase() => {
            chars.all(|c| c == '_' || c.is_ascii_lowercase() || c.is_ascii_digit())
        }
        _ => false,
    }
}

impl FunctionType {
    /// Whether `v` is a value of this type (`any` accepts everything).
    pub fn accepts(self, v: &serde_json::Value) -> bool {
        match self {
            FunctionType::Number => v.is_number(),
            FunctionType::String => v.is_string(),
            FunctionType::Bool => v.is_boolean(),
            FunctionType::Any => true,
        }
    }
}

impl FunctionSignature {
    /// Message mode (D497): no declared output → one input, one output, the
    /// returned value is the message. Otherwise one port per output.
    pub fn is_message(&self) -> bool {
        self.outputs.is_empty()
    }

    /// Server-side check of the declared interface: valid unique names,
    /// defaults matching their type.
    pub fn validate(&self) -> Result<(), SignatureError> {
        let bad = |field: String, code: &str| SignatureError {
            field,
            code: code.to_string(),
        };
        let mut seen = std::collections::BTreeSet::new();
        for (i, input) in self.inputs.iter().enumerate() {
            if !is_valid_port_name(&input.name) {
                return Err(bad(
                    format!("inputs.{i}.name"),
                    crate::err_codes::FIELD_INVALID,
                ));
            }
            if !seen.insert(input.name.as_str()) {
                return Err(bad(
                    format!("inputs.{i}.name"),
                    crate::err_codes::FIELD_UNIQUE,
                ));
            }
            if input.default.as_ref().is_some_and(|d| !input.ty.accepts(d)) {
                return Err(bad(
                    format!("inputs.{i}.default"),
                    crate::err_codes::FIELD_INVALID,
                ));
            }
        }
        let mut seen = std::collections::BTreeSet::new();
        for (i, output) in self.outputs.iter().enumerate() {
            if !is_valid_port_name(&output.name) {
                return Err(bad(
                    format!("outputs.{i}.name"),
                    crate::err_codes::FIELD_INVALID,
                ));
            }
            if !seen.insert(output.name.as_str()) {
                return Err(bad(
                    format!("outputs.{i}.name"),
                    crate::err_codes::FIELD_UNIQUE,
                ));
            }
        }
        Ok(())
    }
}

// ───────────────────────── Bindings & msg paths ─────────────────────────

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
///   declared outputs** (missing key = silent port) when outputs are
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
    /// Snapshot of the declared inputs/outputs of the pinned version —
    /// drives the inspector and the test panel.
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
    /// Declared inputs (one input anchor each); none = one plain input.
    #[serde(default)]
    pub inputs: Vec<FunctionInput>,
    /// Declared outputs (one port each); none = message mode (D497).
    #[serde(default)]
    pub outputs: Vec<FunctionOutput>,
    #[serde(default)]
    pub note: Option<String>,
}

/// Saves a new version (append-only). Optimistic concurrency:
/// `expected_version_number` must be the current version, else 409.
/// `name`/`description` update the metadata; with `code`, `inputs` or
/// `outputs`, the save creates a new version (missing parts are carried
/// from the current version).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct SaveFunctionVersion {
    pub expected_version_number: i64,
    #[serde(default)]
    pub code: Option<String>,
    #[serde(default)]
    pub inputs: Option<Vec<FunctionInput>>,
    #[serde(default)]
    pub outputs: Option<Vec<FunctionOutput>>,
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
    #[serde(default)]
    pub inputs: Vec<FunctionInput>,
    #[serde(default)]
    pub outputs: Vec<FunctionOutput>,
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
    /// Declared outputs: route the Starlark return (the JS wrapper already
    /// embeds them).
    #[serde(default)]
    pub outputs: Vec<FunctionOutput>,
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
    fn signature_validation() {
        let input = |name: &str, ty, default| FunctionInput {
            name: name.into(),
            ty,
            default,
            desc: None,
        };
        let output = |name: &str| FunctionOutput {
            name: name.into(),
            ty: FunctionType::Any,
            desc: None,
        };
        let ok = FunctionSignature {
            inputs: vec![input("t", FunctionType::Number, Some(json!(20)))],
            outputs: vec![output("heating")],
        };
        assert!(ok.validate().is_ok());
        assert!(!ok.is_message());
        assert!(FunctionSignature::default().is_message());
        let err = |sig: FunctionSignature| sig.validate().unwrap_err();
        let e = err(FunctionSignature {
            inputs: vec![input("1abc", FunctionType::Any, None)],
            outputs: vec![],
        });
        assert_eq!(
            (e.field.as_str(), e.code.as_str()),
            ("inputs.0.name", "invalid")
        );
        let e = err(FunctionSignature {
            inputs: vec![input("a", FunctionType::Number, Some(json!("x")))],
            outputs: vec![],
        });
        assert_eq!(e.field, "inputs.0.default");
        let e = err(FunctionSignature {
            inputs: vec![],
            outputs: vec![output("a"), output("a")],
        });
        assert_eq!(
            (e.field.as_str(), e.code.as_str()),
            ("outputs.1.name", "unique")
        );
    }

    #[test]
    fn resolve_msg_path_paths() {
        let msg = json!({"payload": {"t": [10, 20]}, "topic": "a"});
        assert_eq!(resolve_msg_path(&msg, "payload.t.1"), Some(&json!(20)));
        assert_eq!(resolve_msg_path(&msg, "topic"), Some(&json!("a")));
        assert_eq!(resolve_msg_path(&msg, "payload.t.9"), None);
        assert_eq!(resolve_msg_path(&msg, "payload.x.y"), None);
    }

    #[test]
    fn insert_msg_path_creates_levels() {
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
    fn build_inputs_topic_routes_the_payload() {
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
    fn build_input_tag_func_stamps_the_topic() {
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
        assert!(js.contains("__pnex_inputs"), "inputs object");
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
    fn build_js_wrapper_routes_return_by_outputs() {
        let fout = |name: &str| FunctionOutput {
            name: name.into(),
            ty: FunctionType::Bool,
            desc: None,
        };
        let code = "function handle(inputs, msg) { return {out_bool: true}; }";
        let js = build_js_wrapper(code, &[], &[fout("out_bool"), fout("out_temp")]);
        // Key→value object routed per port, in declared output order.
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
            "no routing without declared outputs"
        );
        assert!(
            js.contains("return [__pnex_out(msg, __pnex_r)];"),
            "historical return port 0"
        );
    }

    #[test]
    fn json_split_generates_fan_out_per_key() {
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
    fn json_split_generates_named_ports() {
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
    fn json_merge_generates_context_accumulation() {
        let js = build_json_merge_func("ma clé");
        assert!(
            js.contains("context.get(\"__pnex_merge\")"),
            "accumulator read"
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
