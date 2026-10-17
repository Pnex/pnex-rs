//! Client API du référentiel WiFi (`pnex_core::edge`) — wizard device.

use crate::api::client;
use crate::api::error::ApiError;
use pnex_core::{WifiCredential, WifiCredentialInput};

/// `GET /api/v1/edge/wifi-credentials` — every credential of the org
/// (all pages).
pub async fn list() -> Result<Vec<WifiCredential>, ApiError> {
    client::request_all("/api/v1/edge/wifi-credentials").await
}

/// `POST /api/v1/edge/wifi-credentials` — upsert (200 ou 201, même
/// traitement côté front).
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

/// `DELETE /api/v1/edge/wifi-credentials/{id}` — 204 sans corps
/// (`request_opt` : un `request` classique lèverait « réponse vide »).
pub async fn delete(id: i64) -> Result<(), ApiError> {
    client::request_opt::<()>(
        reqwest::Method::DELETE,
        &format!("/api/v1/edge/wifi-credentials/{id}"),
        None,
    )
    .await?;
    Ok(())
}
