pub mod ai;
pub mod annotation_layers;
/// Profils de board (catalogue /api/v1/boards — éditeur pinout + wizard).
pub mod boards;
pub mod builds;
pub mod camera_recordings;
/// Cameras settings + recorded segments API (D76/D79).
pub mod cameras;
pub mod cases;
pub mod controls;
pub mod dashboard;
pub mod dashboards;
pub mod device_link;
pub mod devices;
pub mod doc_comments;
pub mod docs;
/// Référentiels Edge — credentials WiFi + hosts serveur PNeX (wizard device).
pub mod edge_agents;
pub mod edge_refs;
/// JSON events stored as OpenObserve logs (D84).
pub mod events;
/// Browser agents API (extension-collector.md D254–D269).
pub mod extension;
pub mod firmware_projects;
pub mod flow_cluster;
pub mod flows;
pub mod fluid_mixtures;
pub mod functions;
pub mod geo;
/// Global typeahead across the org's objects (D69).
pub mod global_search;
pub mod health;
/// Access management API (iam.md §8).
pub mod iam;
pub mod internal_flow;
/// Endpoint interne de livraison websocket (runtime de flows / loopback).
pub mod internal_notify;
/// Time ranges (media-ingest.md D169).
pub mod lab;
pub mod mcp;
pub mod media;
/// Vision model registry (D81) + internal model fetch for the flow runtime.
pub mod media_ingest;
pub mod memory;
pub mod meta;
pub mod ml_models;
/// Notifications (D49–D54) — CRUD canaux/templates + test/preview/journal.
pub mod notify;
pub mod oauth2;
pub mod ontology;
pub mod ontology_imports;
pub mod orgs;
pub mod ota;
pub mod pagination;
pub mod pins;
pub mod pois;
pub mod proposals;
pub mod public_tours;
/// Couche d'organisation transverse (D42) — endpoints kind-agnostiques.
pub mod resources;
/// Org secrets vault (D110–D118).
pub mod secrets;
/// Site navigation rules, test runs and their journal (site-recipes.md D323–D326).
pub mod site_recipes;
pub mod stitch_jobs;
/// System: O2 retention/cleanup + platform status (D72).
pub mod system;
/// Topic taxonomies (media-ingest.md D168).
pub mod taxonomies;
pub mod thermo;
pub mod time_ranges;
pub mod tours;
pub mod user_info;
pub mod visualization;
pub mod viz_widgets;
pub mod voice_id;
pub mod web_presence;
pub mod web_search;
/// Camera uplink + browser live view (camera-video.md D73/D75).
pub mod ws_camera;
pub mod ws_device;
pub mod ws_doc;
/// Browser agent link `/ws/extension` (D262).
pub mod ws_extension;
/// Media link of capture boxes (media-ingest.md D160, lot 6b).
pub mod ws_media;
/// Bus WS des notifications (canal `websocket`, D51).
pub mod ws_notify;
pub mod ws_ticket;

/// Error answer of the API: machine `code` (resolved by the frontend to
/// `err-<code>`), canonical English `description`, optional fluent `args`.
pub(crate) fn coded_error(
    status: axum::http::StatusCode,
    code: &str,
    description: impl Into<String>,
    args: Option<serde_json::Value>,
) -> loco_rs::Error {
    loco_rs::Error::CustomError(
        status,
        loco_rs::controller::ErrorDetail {
            error: Some(code.to_string()),
            description: Some(description.into()),
            errors: args.map(|a| serde_json::json!({ "args": a })),
        },
    )
}

/// `field-invalid` answer (D239): `errors.fields = {"<field>": "<token>"}`,
/// machine tokens (`required`, `max_length:255`…).
pub(crate) fn fields_error(
    status: axum::http::StatusCode,
    fields: serde_json::Map<String, serde_json::Value>,
) -> axum::response::Response {
    use axum::response::IntoResponse;
    let body = loco_rs::controller::ErrorDetail {
        error: Some(pnex_core::err_codes::FIELD_INVALID_BODY.to_string()),
        description: Some("one or more fields are invalid".to_string()),
        errors: Some(serde_json::json!({ "fields": fields })),
    };
    (status, axum::Json(body)).into_response()
}

/// Single-field `field-invalid` 400.
pub(crate) fn field_error(field: &str, token: &str) -> axum::response::Response {
    let mut fields = serde_json::Map::new();
    fields.insert(field.to_string(), token.into());
    fields_error(axum::http::StatusCode::BAD_REQUEST, fields)
}

/// Document violations 400 (D239): `code` names the document kind
/// (`flow-invalid`, `dashboard-invalid`…), `errors.violations` the list.
pub(crate) fn violations_error(
    code: &str,
    violations: impl serde::Serialize,
) -> axum::response::Response {
    use axum::response::IntoResponse;
    let body = loco_rs::controller::ErrorDetail {
        error: Some(code.to_string()),
        description: Some("the document is invalid".to_string()),
        errors: Some(serde_json::json!({ "violations": violations })),
    };
    (axum::http::StatusCode::BAD_REQUEST, axum::Json(body)).into_response()
}

/// Axum extractor rejections (unreadable JSON body, unknown enum value,
/// bad query or path) answer `text/plain`: rewrite them into the
/// contract's `ErrorDetail` (D239) with code `request-invalid`, the
/// rejection text as description.
pub(crate) async fn rejection_to_error_detail(
    req: axum::extract::Request,
    next: axum::middleware::Next,
) -> axum::response::Response {
    use axum::http::{header::CONTENT_TYPE, StatusCode};
    use axum::response::IntoResponse;
    let res = next.run(req).await;
    let rejection_status = [
        StatusCode::BAD_REQUEST,
        StatusCode::PAYLOAD_TOO_LARGE,
        StatusCode::UNSUPPORTED_MEDIA_TYPE,
        StatusCode::UNPROCESSABLE_ENTITY,
    ]
    .contains(&res.status());
    let plain = res
        .headers()
        .get(CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|ct| ct.starts_with("text/plain"));
    if !(rejection_status && plain) {
        return res;
    }
    let status = res.status();
    let text = axum::body::to_bytes(res.into_body(), 16 * 1024)
        .await
        .unwrap_or_default();
    let body = loco_rs::controller::ErrorDetail {
        error: Some(pnex_core::err_codes::REQUEST_INVALID.to_string()),
        description: Some(String::from_utf8_lossy(&text).into_owned()),
        errors: None,
    };
    (status, axum::Json(body)).into_response()
}
