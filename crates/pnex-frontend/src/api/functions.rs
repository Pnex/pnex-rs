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

/// `GET /api/v1/functions?limit=200` — registre de l'org (le picker et la
/// liste ne paginent pas ; recherche côté client).
pub async fn list() -> Result<Vec<FunctionSummary>, ApiError> {
    let page: Paginated<FunctionSummary> =
        client::request(reqwest::Method::GET, "/api/v1/functions?limit=200", None).await?;
    Ok(page.results)
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
/// est périmé, 400 champ-par-champ (`{"code": "ligne n : …"}`).
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

/// `DELETE /api/v1/functions/{id}` — 204, ou 409 `function_in_use` avec la
/// liste des flows déployés qui la référencent.
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

/// Liste des flows déployés qui référencent une fonction (corps du 409
/// `function_in_use`) — pour le toast de suppression.
pub fn referencing_flows(err: &ApiError) -> Option<Vec<(i64, String, i64)>> {
    let flows = err
        .body
        .as_ref()?
        .get("referencing_flows")?
        .as_array()?
        .iter()
        .filter_map(|f| {
            Some((
                f.get("flow_id")?.as_i64()?,
                f.get("flow_name")?.as_str()?.to_string(),
                f.get("version_number")?.as_i64()?,
            ))
        })
        .collect::<Vec<_>>();
    (!flows.is_empty()).then_some(flows)
}
