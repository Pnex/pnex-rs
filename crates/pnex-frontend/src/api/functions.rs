//! API client of the "Functions" registry ("Automation" group) — versioned
//! user functions (js | starlark), live test. Types
//! `pnex_core::functions` (shared source of truth, wasm32); `save_version`
//! carries optimistic concurrency (`expected_version_number`, 409 if stale).

use pnex_core::{
    CreateFunction, FunctionDetail, FunctionSummary, FunctionTestRequest, FunctionTestResponse,
    FunctionValidateRequest, FunctionValidateResponse, FunctionVersionSummary, Paginated,
    SaveFunctionVersion,
};

use crate::api::client;
use crate::api::error::ApiError;

/// `GET /api/v1/functions` (all pages) — the org's registry (the picker and
/// the list do not paginate; client-side search).
pub async fn list() -> Result<Vec<FunctionSummary>, ApiError> {
    list_labeled(None).await
}

/// Same as [`list`], filtered by an effective label (`name` or `name:value`, D42).
pub async fn list_labeled(label: Option<String>) -> Result<Vec<FunctionSummary>, ApiError> {
    client::request_all(&labeled_path("/api/v1/functions", label)).await
}

/// Appends `?label=` when a non-empty label filter is set.
fn labeled_path(path: &str, label: Option<String>) -> String {
    match label.filter(|v| !v.is_empty()) {
        Some(v) => format!("{path}?label={}", crate::api::tours::urlencode(&v)),
        None => path.to_string(),
    }
}

/// `POST /api/v1/functions` — creation (function + version 1, code =
/// skeleton of the chosen language).
pub async fn create(input: CreateFunction) -> Result<FunctionDetail, ApiError> {
    client::request(
        reqwest::Method::POST,
        "/api/v1/functions",
        Some(serde_json::to_value(input).unwrap_or_default()),
    )
    .await
}

/// `GET /api/v1/functions/{id}` — detail (code + interface of the current
/// version).
pub async fn detail(id: i64) -> Result<FunctionDetail, ApiError> {
    client::request(
        reqwest::Method::GET,
        &format!("/api/v1/functions/{id}"),
        None,
    )
    .await
}

/// `PATCH /api/v1/functions/{id}` — saves a **new version**
/// (append-only) and/or the metadata; 409 if `expected_version_number`
/// is stale, 400 `field-invalid`.
pub async fn save_version(
    id: i64,
    params: SaveFunctionVersion,
) -> Result<FunctionDetail, ApiError> {
    client::request(
        reqwest::Method::PATCH,
        &format!("/api/v1/functions/{id}"),
        Some(serde_json::to_value(params).unwrap_or_default()),
    )
    .await
}

/// `GET /api/v1/functions/{id}/versions` — history, version_number DESC.
pub async fn versions(
    id: i64,
    limit: i64,
    offset: i64,
) -> Result<Paginated<FunctionVersionSummary>, ApiError> {
    client::request(
        reqwest::Method::GET,
        &format!("/api/v1/functions/{id}/versions?limit={limit}&offset={offset}"),
        None,
    )
    .await
}

/// Every version of a function (all pages): the version picker of the
/// flow inspector must offer any pinned version.
pub async fn versions_all(id: i64) -> Result<Vec<FunctionVersionSummary>, ApiError> {
    client::request_all(&format!("/api/v1/functions/{id}/versions")).await
}

/// `DELETE /api/v1/functions/{id}`: 204, or 409 `function-in-use` naming the
/// deployed flows that use it (`args.flows`).
pub async fn delete(id: i64) -> Result<(), ApiError> {
    client::request_opt::<serde_json::Value>(
        reqwest::Method::DELETE,
        &format!("/api/v1/functions/{id}"),
        None,
    )
    .await
    .map(|_| ())
}

/// `POST /api/v1/functions/{id}/test` — sandboxed run by the runtime
/// (ad-hoc code or pinned version); 503 = runtime unavailable.
pub async fn test(id: i64, params: FunctionTestRequest) -> Result<FunctionTestResponse, ApiError> {
    client::request(
        reqwest::Method::POST,
        &format!("/api/v1/functions/{id}/test"),
        Some(serde_json::to_value(params).unwrap_or_default()),
    )
    .await
}

/// `POST /api/v1/functions/validate` — compile-only validation by the
/// runtime (editor error bar); 503 = runtime unavailable.
pub async fn validate(
    params: FunctionValidateRequest,
) -> Result<FunctionValidateResponse, ApiError> {
    client::request(
        reqwest::Method::POST,
        "/api/v1/functions/validate",
        Some(serde_json::to_value(params).unwrap_or_default()),
    )
    .await
}
