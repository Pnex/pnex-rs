//! API client of the WiFi registry (`pnex_core::edge`) — device wizard.

use crate::api::client;
use crate::api::error::ApiError;
use pnex_core::{WifiCredential, WifiCredentialInput};

/// `GET /api/v1/edge/wifi-credentials` — every credential of the org
/// (all pages).
pub async fn list() -> Result<Vec<WifiCredential>, ApiError> {
    client::request_all("/api/v1/edge/wifi-credentials").await
}

/// `POST /api/v1/edge/wifi-credentials` — upsert (200 or 201, same
/// handling on the front).
pub async fn create(input: WifiCredentialInput) -> Result<WifiCredential, ApiError> {
    client::request(
        reqwest::Method::POST,
        "/api/v1/edge/wifi-credentials",
        Some(serde_json::to_value(input).unwrap_or_default()),
    )
    .await
}

/// `PUT /api/v1/edge/wifi-credentials/{id}` — in-place edit (id kept):
/// SSID rename and/or password. 409 `edge-ssid-taken` when the target SSID
/// is used by another entry of the org.
pub async fn update(id: i64, input: WifiCredentialInput) -> Result<WifiCredential, ApiError> {
    client::request(
        reqwest::Method::PUT,
        &format!("/api/v1/edge/wifi-credentials/{id}"),
        Some(serde_json::to_value(input).unwrap_or_default()),
    )
    .await
}

/// `DELETE /api/v1/edge/wifi-credentials/{id}` — 204 without body
/// (`request_opt`: a classic `request` would raise "empty response").
pub async fn delete(id: i64) -> Result<(), ApiError> {
    client::request_opt::<()>(
        reqwest::Method::DELETE,
        &format!("/api/v1/edge/wifi-credentials/{id}"),
        None,
    )
    .await?;
    Ok(())
}
