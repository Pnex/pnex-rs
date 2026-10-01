//! Org secrets vault API client (`controllers/secrets.rs`, D110–D117).
//! No call ever returns a value.

use pnex_core::{OrgSecret, OrgSecretInput, Paginated};
use uuid::Uuid;

use crate::api::client;
use crate::api::error::ApiError;
use crate::api::media::urlencode;

/// `GET /api/v1/secrets` — paginated, optional name search.
pub async fn list(
    search: Option<&str>,
    limit: i64,
    offset: i64,
) -> Result<Paginated<OrgSecret>, ApiError> {
    let mut q = format!("?limit={limit}&offset={offset}");
    if let Some(s) = search.filter(|s| !s.trim().is_empty()) {
        q.push_str(&format!("&search={}", urlencode(s.trim())));
    }
    client::request(reqwest::Method::GET, &format!("/api/v1/secrets{q}"), None).await
}

/// `POST /api/v1/secrets` — value required.
pub async fn create(input: &OrgSecretInput) -> Result<OrgSecret, ApiError> {
    let value = serde_json::to_value(input).map_err(|e| ApiError::new(e.to_string()))?;
    client::request(reqwest::Method::POST, "/api/v1/secrets", Some(value)).await
}

/// `PUT /api/v1/secrets/{id}` — `value: None` keeps the current value.
pub async fn update(id: Uuid, input: &OrgSecretInput) -> Result<OrgSecret, ApiError> {
    let value = serde_json::to_value(input).map_err(|e| ApiError::new(e.to_string()))?;
    client::request(
        reqwest::Method::PUT,
        &format!("/api/v1/secrets/{id}"),
        Some(value),
    )
    .await
}

/// `DELETE /api/v1/secrets/{id}` — 204, 409 `secret-in-use` while used.
pub async fn delete(id: Uuid) -> Result<(), ApiError> {
    client::request_opt::<()>(
        reqwest::Method::DELETE,
        &format!("/api/v1/secrets/{id}"),
        None,
    )
    .await?;
    Ok(())
}
