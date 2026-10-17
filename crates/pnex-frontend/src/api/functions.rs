//! Client API du registre « Fonctions » (groupe « Automation ») — fonctions
//! utilisateur versionnées (js | starlark), test en live. Types
//! `pnex_core::functions` (source de vérité partagée, wasm32) ; `save_version`
//! porte la concurrence optimiste (`expected_version_number`, 409 si périmé).

use pnex_core::{
    CreateFunction, FunctionDetail, FunctionSummary, FunctionTestRequest, FunctionTestResponse,
    FunctionValidateRequest, FunctionValidateResponse, FunctionVersionSummary, Paginated,
    SaveFunctionVersion,
};

use crate::api::client;
use crate::api::error::ApiError;

/// `GET /api/v1/functions` (all pages) — registre de l'org (le picker et la
/// liste ne paginent pas ; recherche côté client).
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

/// `POST /api/v1/functions` — création (fonction + version 1, code =
/// squelette du langage choisi).
pub async fn create(input: CreateFunction) -> Result<FunctionDetail, ApiError> {
    client::request(
        reqwest::Method::POST,
        "/api/v1/functions",
        Some(serde_json::to_value(input).unwrap_or_default()),
    )
    .await
}

/// `GET /api/v1/functions/{id}` — détail (code + interface de la version
/// courante).
pub async fn detail(id: i64) -> Result<FunctionDetail, ApiError> {
    client::request(
        reqwest::Method::GET,
        &format!("/api/v1/functions/{id}"),
        None,
    )
    .await
}

/// `PATCH /api/v1/functions/{id}` — enregistre une **nouvelle version**
/// (append-only) et/ou les métadonnées ; 409 si `expected_version_number`
/// est périmé, 400 `field-invalid` or `function-directive-invalid`.
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

/// `GET /api/v1/functions/{id}/versions` — historique, version_number DESC.
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

/// `POST /api/v1/functions/{id}/test` — exécution sandboxée par le runtime
/// (code ad-hoc ou version épinglée) ; 503 = runtime indisponible.
pub async fn test(id: i64, params: FunctionTestRequest) -> Result<FunctionTestResponse, ApiError> {
    client::request(
        reqwest::Method::POST,
        &format!("/api/v1/functions/{id}/test"),
        Some(serde_json::to_value(params).unwrap_or_default()),
    )
    .await
}

/// `POST /api/v1/functions/validate` — validation compile-only par le
/// runtime (barre d'erreurs de l'éditeur) ; 503 = runtime indisponible.
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
