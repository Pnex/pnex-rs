//! MCP server (D245): the OpenAPI spec becomes the tools, served at
//! `POST /api/v1/mcp` (Streamable HTTP, JSON answers, stateless). Only the
//! operations of [`exposure::EXPOSED`] are tools. A call is replayed
//! in-process on the app router with the caller's own `Authorization`, so
//! it gets exactly the caller's rights and the handlers' validation.

pub use pnex_api_contract::exposure;

use std::sync::LazyLock;

use axum::body::Body;
use axum::http::{header, HeaderMap, Method, Request};
use percent_encoding::{utf8_percent_encode, NON_ALPHANUMERIC};
use serde_json::{json, Map, Value};
use tower::ServiceExt;

/// Final app router, stored at boot (`App::after_routes`) for the replays.
#[derive(Clone)]
pub struct AppRouter(pub axum::Router);

/// Protocol revisions this server speaks, newest first.
pub const PROTOCOL_VERSIONS: &[&str] = &["2025-06-18", "2025-03-26", "2024-11-05"];

const COMPONENTS: &str = "#/components/schemas/";
const ORG_HEADER: &str = "X-Org-Id";
/// Largest answer body handed back to the model.
const MAX_ANSWER: usize = 4 * 1024 * 1024;
/// Answer fields never shown to a model, whatever the route (R4, R16).
const REDACTED: &[&str] = &[
    "access_token",
    "api_key",
    "auth_secret",
    "device_token",
    "encryption_key",
    "id_token",
    "password",
    "refresh_token",
    "secrets",
    "share_token",
    "token",
];

struct Tool {
    method: Method,
    path: String,
    /// `(name, in)` with `in` = `path` | `query` | `header`.
    params: Vec<(String, String)>,
    def: Value,
}

static TOOLS: LazyLock<Vec<(String, Tool)>> = LazyLock::new(build_tools);

/// `tools/list` answer: every exposed operation.
pub fn tool_list() -> Value {
    Value::Array(TOOLS.iter().map(|(_, t)| t.def.clone()).collect())
}

fn build_tools() -> Vec<(String, Tool)> {
    let spec: Value = serde_json::from_str(&crate::openapi::spec_json()).expect("spec parses");
    let schemas = &spec["components"]["schemas"];
    let mut tools = Vec::new();
    for (path, item) in spec["paths"].as_object().into_iter().flatten() {
        for (method, op) in item.as_object().into_iter().flatten() {
            let Some(name) = op["operationId"].as_str() else {
                continue;
            };
            if !exposure::EXPOSED.contains(&name) {
                continue;
            }
            let method = Method::from_bytes(method.to_uppercase().as_bytes()).expect("method");
            tools.push((name.to_string(), tool(name, method, path, op, schemas)));
        }
    }
    tools
}

fn tool(name: &str, method: Method, path: &str, op: &Value, schemas: &Value) -> Tool {
    let mut props = Map::new();
    let mut required = Vec::new();
    let mut params = Vec::new();
    for p in op["parameters"].as_array().into_iter().flatten() {
        let (Some(pname), Some(loc)) = (p["name"].as_str(), p["in"].as_str()) else {
            continue;
        };
        let mut schema = p["schema"].clone();
        let org = loc == "header" && pname.eq_ignore_ascii_case(ORG_HEADER);
        let doc = match (p["description"].as_str(), org) {
            (_, true) => Some(
                "Id of the organisation (see `user_info_get`); defaults to the \
                 X-Org-Id header of the MCP connection."
                    .to_string(),
            ),
            (Some(d), _) => Some(d.to_string()),
            _ => None,
        };
        if let (Some(d), Some(obj)) = (doc, schema.as_object_mut()) {
            obj.insert("description".into(), d.into());
        }
        props.insert(pname.to_string(), schema);
        if p["required"] == true && !org {
            required.push(pname.to_string());
        }
        params.push((pname.to_string(), loc.to_string()));
    }
    if let Some(body) = op["requestBody"].as_object() {
        props.insert(
            "body".into(),
            body["content"]["application/json"]["schema"].clone(),
        );
        if body.get("required") == Some(&Value::Bool(true)) {
            required.push("body".into());
        }
    }
    let mut input = json!({ "type": "object", "properties": props, "required": required });
    let defs = defs_of(&input, schemas);
    if !defs.is_empty() {
        input["$defs"] = Value::Object(defs);
    }
    rewrite_refs(&mut input);

    let summary = op["summary"].as_str().unwrap_or(name);
    let description = match op["description"].as_str() {
        Some(d) => format!("{summary}\n\n{d}"),
        None => summary.to_string(),
    };
    let read_only = method == Method::GET || path.ends_with("/query");
    Tool {
        def: json!({
            "name": name,
            "title": summary,
            "description": description,
            "inputSchema": input,
            "annotations": { "readOnlyHint": read_only, "destructiveHint": false },
        }),
        method,
        path: path.to_string(),
        params,
    }
}

/// Component schemas reached from `value`, transitively.
fn defs_of(value: &Value, schemas: &Value) -> Map<String, Value> {
    let mut pending = Vec::new();
    refs(value, &mut pending);
    let mut defs = Map::new();
    while let Some(name) = pending.pop() {
        if defs.contains_key(&name) {
            continue;
        }
        let def = schemas[&name].clone();
        refs(&def, &mut pending);
        defs.insert(name, def);
    }
    defs
}

fn refs(value: &Value, out: &mut Vec<String>) {
    match value {
        Value::Object(map) => {
            for (k, v) in map {
                match v.as_str().and_then(|r| r.strip_prefix(COMPONENTS)) {
                    Some(name) if k == "$ref" => out.push(name.to_string()),
                    _ => refs(v, out),
                }
            }
        }
        Value::Array(items) => items.iter().for_each(|v| refs(v, out)),
        _ => {}
    }
}

fn rewrite_refs(value: &mut Value) {
    match value {
        Value::Object(map) => {
            for (k, v) in map.iter_mut() {
                match v {
                    Value::String(r) if k == "$ref" => {
                        if let Some(name) = r.strip_prefix(COMPONENTS) {
                            *r = format!("#/$defs/{name}");
                        }
                    }
                    _ => rewrite_refs(v),
                }
            }
        }
        Value::Array(items) => items.iter_mut().for_each(rewrite_refs),
        _ => {}
    }
}

/// JSON-RPC error of a `tools/call` that never reached a handler.
pub type CallError = (i64, String);

/// `tools/call`: replays the operation with the caller's credentials and
/// answers the MCP tool result (`isError` on a non-2xx).
pub async fn call(
    router: axum::Router,
    headers: &HeaderMap,
    params: &Value,
) -> Result<Value, CallError> {
    let name = params["name"].as_str().unwrap_or_default();
    let Some((_, tool)) = TOOLS.iter().find(|(n, _)| n == name) else {
        return Err((-32602, format!("unknown tool `{name}`")));
    };
    let req = request(tool, params, headers)?;
    let res = router.oneshot(req).await.expect("router is infallible");
    let status = res.status();
    let text = match axum::body::to_bytes(res.into_body(), MAX_ANSWER).await {
        Ok(bytes) => match serde_json::from_slice::<Value>(&bytes) {
            Ok(mut v) => {
                redact(&mut v);
                v.to_string()
            }
            Err(_) => String::from_utf8_lossy(&bytes).into_owned(),
        },
        Err(_) => format!("answer larger than {MAX_ANSWER} bytes, narrow the request"),
    };
    let text = match (status.is_success(), text.is_empty()) {
        (true, false) => text,
        (true, true) => format!("HTTP {}", status.as_u16()),
        (false, _) => format!("HTTP {}: {text}", status.as_u16()),
    };
    Ok(json!({
        "content": [{ "type": "text", "text": text }],
        "isError": !status.is_success(),
    }))
}

/// The HTTP request of a tool call. Sync: the query serializer is not Send.
fn request(tool: &Tool, params: &Value, headers: &HeaderMap) -> Result<Request<Body>, CallError> {
    let empty = Map::new();
    let args = params["arguments"].as_object().unwrap_or(&empty);

    let mut path = tool.path.clone();
    let mut query = form_urlencoded::Serializer::new(String::new());
    let mut req = Request::builder().method(tool.method.clone());
    let mut org_set = false;
    for (pname, loc) in &tool.params {
        let Some(value) = args.get(pname).filter(|v| !v.is_null()) else {
            if loc == "path" {
                return Err((-32602, format!("missing argument `{pname}`")));
            }
            continue;
        };
        match loc.as_str() {
            "path" => {
                let raw = scalar(value);
                let enc = utf8_percent_encode(&raw, NON_ALPHANUMERIC).to_string();
                path = path.replace(&format!("{{{pname}}}"), &enc);
            }
            "query" => match value {
                Value::Array(items) => items.iter().for_each(|v| {
                    query.append_pair(pname, &scalar(v));
                }),
                v => {
                    query.append_pair(pname, &scalar(v));
                }
            },
            _ => {
                org_set |= pname.eq_ignore_ascii_case(ORG_HEADER);
                req = req.header(pname.as_str(), scalar(value));
            }
        }
    }
    if let (false, Some(org)) = (org_set, headers.get(ORG_HEADER)) {
        req = req.header(ORG_HEADER, org);
    }
    if let Some(auth) = headers.get(header::AUTHORIZATION) {
        req = req.header(header::AUTHORIZATION, auth);
    }
    let qs = query.finish();
    let uri = if qs.is_empty() {
        path
    } else {
        format!("{path}?{qs}")
    };
    let body = match args.get("body") {
        Some(b) => {
            req = req.header(header::CONTENT_TYPE, "application/json");
            Body::from(b.to_string())
        }
        None => Body::empty(),
    };
    req.uri(uri)
        .body(body)
        .map_err(|e| (-32602, format!("invalid arguments: {e}")))
}

fn scalar(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        v => v.to_string(),
    }
}

fn redact(value: &mut Value) {
    match value {
        Value::Object(map) => {
            for (k, v) in map.iter_mut() {
                if REDACTED.contains(&k.as_str()) && !v.is_null() {
                    *v = Value::String("[redacted]".into());
                } else {
                    redact(v);
                }
            }
        }
        Value::Array(items) => items.iter_mut().for_each(redact),
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redact_hides_secrets_at_any_depth() {
        let mut v = json!({"results": [{"device_token": "abc", "name": "x", "api_key": null}]});
        redact(&mut v);
        assert_eq!(v["results"][0]["device_token"], "[redacted]");
        assert_eq!(v["results"][0]["name"], "x");
        assert!(v["results"][0]["api_key"].is_null());
    }

    #[test]
    fn tool_schemas_are_self_contained() {
        for (name, tool) in TOOLS.iter() {
            let text = tool.def["inputSchema"].to_string();
            assert!(!text.contains(COMPONENTS), "{name}: dangling component ref");
            let mut local = Vec::new();
            collect_local(&tool.def["inputSchema"], &mut local);
            for r in local {
                assert!(
                    tool.def["inputSchema"]["$defs"].get(&r).is_some(),
                    "{name}: #/$defs/{r} missing"
                );
            }
        }
    }

    fn collect_local(value: &Value, out: &mut Vec<String>) {
        match value {
            Value::Object(map) => map.iter().for_each(|(k, v)| {
                match v.as_str().and_then(|r| r.strip_prefix("#/$defs/")) {
                    Some(name) if k == "$ref" => out.push(name.to_string()),
                    _ => collect_local(v, out),
                }
            }),
            Value::Array(items) => items.iter().for_each(|v| collect_local(v, out)),
            _ => {}
        }
    }
}
