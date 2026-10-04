//! Render-time localization of API errors — code → fluent key.
//!
//! The server keeps a machine code (`pnex_core::err_codes::ALL`) plus a
//! canonical English `description`; this module resolves the display text
//! from the fluent locales at **render time** (`i18n()` requires a reactive
//! render scope — never call from async tasks). Unknown codes (IdP, Rauthy,
//! legacy bodies) fall back to the verbatim `message` for unregistered codes
//! or on any translation failure (missing key, missing arg). Translation
//! never panics here: `try_translate_with_args` errors degrade to the
//! verbatim `message`.

use dioxus_i18n::fluent;
use dioxus_i18n::prelude::*;
use pnex_core::err_codes;

use crate::api::error::ApiError;
use pnex_core::FlowViolation;

/// Convert a JSON object of strings (`{"device": "esp32-1"}`) into
/// [`fluent::FluentArgs`]. Non-string values are stringified. Returns `None`
/// for missing/empty objects so `try_translate_with_args` gets `None`.
fn json_to_args(args: Option<&serde_json::Value>) -> Option<fluent::FluentArgs<'static>> {
    let obj = args?.as_object()?;
    if obj.is_empty() {
        return None;
    }
    let mut out = fluent::FluentArgs::new();
    for (key, value) in obj {
        let text = match value {
            serde_json::Value::String(s) => s.clone(),
            other => other.to_string(),
        };
        out.set(key.clone(), text);
    }
    Some(out)
}

/// Non-panicking translation with optional JSON args; degrades to the raw
/// key when the translation fails (missing key, missing arg, …). Render
/// scope required (uses the i18n context).
pub fn resolve(key: &str, args: Option<&serde_json::Value>) -> String {
    let i18n = i18n();
    let fluent_args = json_to_args(args);
    i18n.try_translate_with_args(key, fluent_args.as_ref())
        .unwrap_or_else(|_| key.to_string())
}

/// True when the value looks like a **machine token** (compact lowercase
/// identifier, optionally `token:data`): `required`, `max_length:255`,
/// `invalid-uuid`. Legacy prose ("This field is required.", French
/// messages) never matches and falls back to verbatim display.
fn is_machine_token(value: &str) -> bool {
    let head = value.split(':').next().unwrap_or(value);
    let head_ok = !head.is_empty()
        && head.chars().next().is_some_and(|c| c.is_ascii_lowercase())
        && head
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '-');
    head_ok && !value.contains(char::is_whitespace)
}

/// Localize a field-status value (`{"name": "<token>"}` single-field shape): token →
/// `err-<kebab>` with the `token:data` suffix as `$value` arg; prose →
/// verbatim. The `field` name is kept for future per-field keys.
pub fn localize_field(_field: &str, value: &str) -> String {
    if !is_machine_token(value) {
        return value.to_owned();
    }
    let (head, tail) = match value.split_once(':') {
        Some((head, tail)) => (head, Some(tail)),
        None => (value, None),
    };
    let key = format!("err-{}", head.replace('_', "-"));
    let args = tail.map(|v| serde_json::json!({ "value": v }));
    resolve(&key, args.as_ref())
}

/// Localize an [`ApiError`]: a 400 `{"violations": […]}` body resolves each
/// violation (joined " ; "); otherwise a registered code → `err-<kebab>`
/// with args; otherwise the verbatim extracted `message`. Also resolves
/// client-side errors built with `ApiError::local` (err_codes client codes).
pub fn localize(err: &ApiError) -> String {
    // 400 {"violations": […]} — violation-level resolution first.
    if let Some(joined) = violations_localized(err) {
        return joined;
    }
    // Single-field shaped body ({"<field>": "<value>"}) — resolve a
    // machine token value; legacy prose displays verbatim.
    let field_token = err.body.as_ref().and_then(|body| {
        let obj = body.as_object()?;
        if obj.len() != 1 {
            return None;
        }
        obj.values().next()?.as_str().map(str::to_owned)
    });
    if let Some(value) = field_token {
        return localize_field("", &value);
    }
    match err.code.as_deref() {
        Some(code) if err_codes::exists(code) => {
            resolve(&err_codes::fluent_key(code), err.args.as_ref())
        }
        _ => err.message.clone(),
    }
}

/// Localize a [`FlowViolation`]: registered code → `err-<kebab>` with
/// `node_id` + `args`; translation failure (unknown code, missing key,
/// missing arg) → verbatim `message`.
pub fn localize_violation(violation: &FlowViolation) -> String {
    let mut args = violation.args.clone().unwrap_or(serde_json::json!({}));
    if let (Some(node_id), Some(obj)) = (violation.node_id.as_deref(), args.as_object_mut()) {
        obj.entry("node_id")
            .or_insert(serde_json::Value::String(node_id.to_owned()));
    }
    if !err_codes::exists(&violation.code) {
        // Flow-check codes (`camera_device_missing`, …) are not server error
        // codes: use their `err-<kebab>` key when the locales carry one,
        // else the verbatim English message.
        let fluent_args = json_to_args(Some(&args));
        return i18n()
            .try_translate_with_args(
                &err_codes::fluent_key(&violation.code),
                fluent_args.as_ref(),
            )
            .unwrap_or_else(|_| violation.message.clone());
    }
    resolve(&err_codes::fluent_key(&violation.code), Some(&args))
}

/// Localize an assistant tool trace line: a coded refusal resolves its
/// `err-<code>` key with `args`, anything else (or a failed translation)
/// shows the verbatim summary. Render scope required.
pub fn localize_tool_summary(
    code: Option<&str>,
    args: Option<&serde_json::Value>,
    summary: &str,
) -> String {
    match code {
        Some(code) if err_codes::exists(code) => {
            let fluent_args = json_to_args(args);
            i18n()
                .try_translate_with_args(&err_codes::fluent_key(code), fluent_args.as_ref())
                .unwrap_or_else(|_| summary.to_owned())
        }
        _ => summary.to_owned(),
    }
}

/// Localize a full API-violations payload (`{"violations": […]}`) — the
/// localized form of `api::flows::violations_message`.
pub fn violations_localized(err: &ApiError) -> Option<String> {
    let violations: Vec<String> = err
        .body
        .as_ref()?
        .get("violations")?
        .as_array()?
        .iter()
        .filter_map(|v| serde_json::from_value::<FlowViolation>(v.clone()).ok())
        .map(|v| localize_violation(&v))
        .collect();
    if violations.is_empty() {
        return None;
    }
    Some(violations.join(" ; "))
}
