//! OpenAPI spec of the HTTP API (`docs/contracts/openapi.json`), source of
//! the SDKs. Two guards:
//! - the committed file equals the spec derived from the handlers
//!   (`task openapi` rewrites it);
//! - every route is documented, or listed in `fixtures/openapi_pending.txt`.
//!   That list only shrinks: a documented route must leave it, and a new
//!   route cannot be added to it — annotate it with `#[utoipa::path]`.

use std::collections::BTreeSet;
use std::path::PathBuf;

const SPEC_FILE: &str = "../../docs/contracts/openapi.json";
const PENDING_FILE: &str = "tests/fixtures/openapi_pending.txt";

/// Routes outside the SDK surface: loco's built-in probes, WebSockets
/// (not describable in OpenAPI) and server-to-server internal endpoints.
fn excluded(route: &str) -> bool {
    let uri = route.split_once(' ').map_or(route, |(_, u)| u);
    uri.starts_with("/_") || uri.starts_with("/ws/") || uri.contains("/internal/")
}

fn manifest() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

#[test]
fn openapi_file_is_up_to_date() {
    let json = pnex_backend::openapi::spec_json();
    let path = manifest().join(SPEC_FILE);
    if std::env::var_os("UPDATE_OPENAPI").is_some() {
        std::fs::write(&path, &json).expect("write openapi.json");
        return;
    }
    let committed = std::fs::read_to_string(&path).unwrap_or_default();
    assert!(
        committed == json,
        "docs/contracts/openapi.json is stale: run `task openapi` and commit it"
    );
}

#[test]
fn every_route_is_documented_or_pending() {
    let spec = pnex_backend::openapi::spec();
    let mut documented = BTreeSet::new();
    for (path, item) in &spec.paths.paths {
        for (method, op) in [
            ("GET", &item.get),
            ("POST", &item.post),
            ("PUT", &item.put),
            ("PATCH", &item.patch),
            ("DELETE", &item.delete),
        ] {
            if op.is_some() {
                documented.insert(format!("{method} {path}"));
            }
        }
    }

    let routes: BTreeSet<String> = pnex_backend::app::app_routes()
        .collect()
        .into_iter()
        .flat_map(|r| {
            r.actions
                .into_iter()
                .filter(|m| m != axum::http::Method::HEAD)
                // An axum wildcard `{*rest}` is a plain `{rest}` in OpenAPI.
                .map(move |m| format!("{m} {}", r.uri.replace("{*", "{")))
        })
        .filter(|r| !excluded(r))
        .collect();

    let pending_text =
        std::fs::read_to_string(manifest().join(PENDING_FILE)).expect("read pending list");
    let pending: BTreeSet<String> = pending_text
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .map(String::from)
        .collect();

    let undocumented: Vec<_> = routes
        .iter()
        .filter(|r| !documented.contains(*r) && !pending.contains(*r))
        .collect();
    let ghosts: Vec<_> = documented.difference(&routes).collect();
    let stale: Vec<_> = pending
        .iter()
        .filter(|p| documented.contains(*p) || !routes.contains(*p))
        .collect();

    assert!(
        undocumented.is_empty(),
        "routes without #[utoipa::path] (annotate them, never add them to {PENDING_FILE}):\n{}",
        undocumented
            .iter()
            .map(|s| s.as_str())
            .collect::<Vec<_>>()
            .join("\n")
    );
    assert!(
        ghosts.is_empty(),
        "documented paths that no route serves (wrong path or method): {ghosts:?}"
    );
    assert!(
        stale.is_empty(),
        "remove from {PENDING_FILE} (documented or gone): {stale:?}"
    );
    assert!(routes.len() > 100, "sterile sweep: {} routes", routes.len());
}

// ── Contract shape guards (api-contract.md D239–D243) ─────────────────────

fn spec_value() -> serde_json::Value {
    serde_json::from_str(&pnex_backend::openapi::spec_json()).expect("spec is JSON")
}

/// `(METHOD path, operation)` of every operation of the spec.
fn operations(spec: &serde_json::Value) -> Vec<(String, &serde_json::Value)> {
    let mut out = Vec::new();
    for (path, item) in spec["paths"].as_object().expect("paths") {
        for (method, op) in item.as_object().expect("path item") {
            if ["get", "post", "put", "patch", "delete"].contains(&method.as_str()) {
                out.push((format!("{} {path}", method.to_uppercase()), op));
            }
        }
    }
    out
}

fn report(what: &str, offenders: Vec<String>) {
    assert!(
        offenders.is_empty(),
        "{what} ({}):\n{}",
        offenders.len(),
        offenders.join("\n")
    );
}

/// D240: a collection is a `Paginated_<T>`, never a bare JSON array.
#[test]
fn no_bare_array_responses() {
    let spec = spec_value();
    let mut offenders = Vec::new();
    for (route, op) in operations(&spec) {
        for (status, response) in op["responses"].as_object().into_iter().flatten() {
            let schema = &response["content"]["application/json"]["schema"];
            if status.starts_with('2') && schema["type"] == "array" {
                offenders.push(format!("{route} → {status}"));
            }
        }
    }
    report(
        "2xx JSON bodies that are bare arrays (use Paginated<T>)",
        offenders,
    );
}

/// OAuth2 token endpoints relay the IdP's refusals in the standard OAuth2
/// error shape (RFC 6749 §5.2, `{error, error_description}`) that OAuth2
/// clients expect — the one documented exception to D239.
fn oauth2_upstream(route: &str) -> bool {
    ["POST /api/v1/oauth2/token", "POST /api/v1/oauth2/refresh"].contains(&route)
}

/// D239: every non-2xx JSON body is `ErrorDetail`.
#[test]
fn error_responses_are_error_detail() {
    let spec = spec_value();
    let mut offenders = Vec::new();
    for (route, op) in operations(&spec) {
        for (status, response) in op["responses"].as_object().into_iter().flatten() {
            if status.starts_with('2') || status.starts_with('3') || oauth2_upstream(&route) {
                continue;
            }
            for (media, content) in response["content"].as_object().into_iter().flatten() {
                if content["schema"]["$ref"] != "#/components/schemas/ErrorDetail" {
                    offenders.push(format!("{route} → {status} {media}"));
                }
            }
        }
    }
    report("error responses that are not ErrorDetail", offenders);
}

/// D241: timestamps (`*_at`) are `date-time`, path ids are int64 or uuid.
#[test]
fn ids_and_timestamps_are_typed() {
    let spec = spec_value();
    let mut offenders = Vec::new();
    for (name, schema) in spec["components"]["schemas"].as_object().expect("schemas") {
        for (field, prop) in schema["properties"].as_object().into_iter().flatten() {
            let string = prop["type"] == "string"
                || prop["type"]
                    .as_array()
                    .is_some_and(|t| t.contains(&serde_json::json!("string")));
            let dated = prop["format"] == "date-time" || prop["format"] == "date";
            if field.ends_with("_at") && string && !dated {
                offenders.push(format!("{name}.{field}: date-time"));
            }
        }
    }
    for (route, op) in operations(&spec) {
        for param in op["parameters"].as_array().into_iter().flatten() {
            let name = param["name"].as_str().unwrap_or_default();
            let schema = &param["schema"];
            let typed = schema["format"] == "int64" || schema["format"] == "uuid";
            // `/resources/{kind}/{id}`: the id of a polymorphic resource is
            // an int64 or a uuid depending on `kind`.
            let polymorphic = route.contains("/resources/{kind}/{id}");
            if param["in"] == "path"
                && (name == "id" || name.ends_with("_id"))
                && !typed
                && !polymorphic
            {
                offenders.push(format!("{route} {{{name}}}: int64 or uuid"));
            }
        }
    }
    report("untyped ids/timestamps", offenders);
}

/// D243: the contract is English (summaries, descriptions, schema docs).
#[test]
fn contract_text_is_english() {
    fn walk(value: &serde_json::Value, at: &str, out: &mut Vec<String>) {
        match value {
            serde_json::Value::Object(map) => {
                for (key, v) in map {
                    let here = format!("{at}/{key}");
                    if let (true, Some(text)) =
                        (key == "summary" || key == "description", v.as_str())
                    {
                        if looks_french(text) {
                            out.push(format!(
                                "{here}: {}",
                                text.chars().take(80).collect::<String>()
                            ));
                        }
                    }
                    walk(v, &here, out);
                }
            }
            serde_json::Value::Array(items) => {
                for (i, v) in items.iter().enumerate() {
                    walk(v, &format!("{at}/{i}"), out);
                }
            }
            _ => {}
        }
    }
    let mut offenders = Vec::new();
    walk(&spec_value(), "", &mut offenders);
    report("French text in the contract", offenders);
}

/// Heuristic: French diacritics or frequent French function words.
fn looks_french(text: &str) -> bool {
    const WORDS: &[&str] = &[
        " le ", " la ", " les ", " des ", " du ", " une ", " est ", " pour ", " avec ", " sans ",
        " sur ", " dans ", " pas ",
    ];
    let lower = format!(" {} ", text.to_lowercase());
    text.chars().any(|c| "éèêàâùûçôîœ".contains(c)) || WORDS.iter().any(|w| lower.contains(w))
}

#[test]
fn french_heuristic() {
    assert!(looks_french("Liste des appareils"));
    assert!(looks_french("créé"));
    assert!(!looks_french(
        "List the devices of the organisation, newest first."
    ));
}

/// Every `$ref` resolves to a component (a type used only through
/// `IntoParams` must be listed in its controller's `components(schemas)`).
#[test]
fn every_ref_resolves() {
    fn walk(value: &serde_json::Value, schemas: &serde_json::Value, out: &mut Vec<String>) {
        match value {
            serde_json::Value::Object(map) => {
                if let Some(r) = map.get("$ref").and_then(|r| r.as_str()) {
                    let name = r.trim_start_matches("#/components/schemas/");
                    if schemas.get(name).is_none() {
                        out.push(r.to_string());
                    }
                }
                map.values().for_each(|v| walk(v, schemas, out));
            }
            serde_json::Value::Array(items) => items.iter().for_each(|v| walk(v, schemas, out)),
            _ => {}
        }
    }
    let spec = spec_value();
    let mut offenders = Vec::new();
    walk(&spec, &spec["components"]["schemas"], &mut offenders);
    offenders.sort();
    offenders.dedup();
    report("dangling $ref", offenders);
}
