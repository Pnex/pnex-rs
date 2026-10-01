//! Custom firmware IDE API (custom-firmware.md D87–D94): projects with
//! append-only revisions, pinned library catalog, compile-only checks,
//! device attachment and custom device commands. Types live in
//! `pnex_core::firmware` (shared with the backend).

use pnex_core::firmware::{
    CreateFirmwareProject, FirmwareCheckStatus, FirmwareProjectDetail, FirmwareProjectSummary,
    FirmwareRevisionSummary, LibCatalogItem, SaveFirmwareRevision,
};
use serde::Deserialize;

use crate::api::client;
use crate::api::error::ApiError;

/// `{count, results}` envelope of the firmware endpoints.
#[derive(Deserialize)]
struct Results<T> {
    results: Vec<T>,
}

pub async fn list() -> Result<Vec<FirmwareProjectSummary>, ApiError> {
    let r: Results<FirmwareProjectSummary> =
        client::request(reqwest::Method::GET, "/api/v1/firmware-projects", None).await?;
    Ok(r.results)
}

pub async fn lib_catalog() -> Result<Vec<LibCatalogItem>, ApiError> {
    let r: Results<LibCatalogItem> = client::request(
        reqwest::Method::GET,
        "/api/v1/firmware-projects/lib-catalog",
        None,
    )
    .await?;
    Ok(r.results)
}

pub async fn create(input: CreateFirmwareProject) -> Result<FirmwareProjectDetail, ApiError> {
    client::request(
        reqwest::Method::POST,
        "/api/v1/firmware-projects",
        Some(serde_json::to_value(input).unwrap_or_default()),
    )
    .await
}

pub async fn detail(id: i64) -> Result<FirmwareProjectDetail, ApiError> {
    client::request(
        reqwest::Method::GET,
        &format!("/api/v1/firmware-projects/{id}"),
        None,
    )
    .await
}

/// New revision and/or metadata — 409 when `expected_revision_number` is
/// stale; 400 `firmware-source-refused` carries per-line `violations`.
pub async fn save(
    id: i64,
    params: SaveFirmwareRevision,
) -> Result<FirmwareProjectDetail, ApiError> {
    client::request(
        reqwest::Method::PATCH,
        &format!("/api/v1/firmware-projects/{id}"),
        Some(serde_json::to_value(params).unwrap_or_default()),
    )
    .await
}

pub async fn revisions(id: i64) -> Result<Vec<FirmwareRevisionSummary>, ApiError> {
    let r: Results<FirmwareRevisionSummary> = client::request(
        reqwest::Method::GET,
        &format!("/api/v1/firmware-projects/{id}/revisions"),
        None,
    )
    .await?;
    Ok(r.results)
}

/// 204, or 409 `firmware-project-in-use` while devices are attached.
pub async fn delete(id: i64) -> Result<(), ApiError> {
    client::request_opt::<serde_json::Value>(
        reqwest::Method::DELETE,
        &format!("/api/v1/firmware-projects/{id}"),
        None,
    )
    .await
    .map(|_| ())
}

/// Starts a compile-only check of the current revision; returns its id.
pub async fn start_check(id: i64) -> Result<String, ApiError> {
    let v: serde_json::Value = client::request(
        reqwest::Method::POST,
        &format!("/api/v1/firmware-projects/{id}/check"),
        Some(serde_json::json!({})),
    )
    .await?;
    Ok(v.get("check_id")
        .and_then(|c| c.as_str())
        .unwrap_or_default()
        .to_string())
}

pub async fn check_status(id: i64, check_id: &str) -> Result<FirmwareCheckStatus, ApiError> {
    client::request(
        reqwest::Method::GET,
        &format!("/api/v1/firmware-projects/{id}/checks/{check_id}"),
        None,
    )
    .await
}

/// Per-line sketch violations of a 400 `firmware-source-refused` body.
pub fn source_violations(err: &ApiError) -> Vec<pnex_core::firmware::SourceViolation> {
    err.body
        .as_ref()
        .and_then(|b| b.get("violations"))
        .and_then(|v| serde_json::from_value(v.clone()).ok())
        .unwrap_or_default()
}
