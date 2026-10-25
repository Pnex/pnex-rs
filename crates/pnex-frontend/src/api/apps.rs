//! Applications of the org (external-apps.md D433): `/api/v1/apps`.

use pnex_core::apps::{AppInput, AppView};
use serde_json::Value;

use crate::api::client;
use crate::api::error::ApiError;

/// `GET /api/v1/apps`.
pub async fn list() -> Result<Vec<AppView>, ApiError> {
    client::request_all("/api/v1/apps").await
}

/// `POST /api/v1/apps`, or `PUT /api/v1/apps/{id}` on its version.
pub async fn save(id: Option<&str>, input: &AppInput) -> Result<AppView, ApiError> {
    let body = serde_json::to_value(input).ok();
    match id {
        None => client::request(reqwest::Method::POST, "/api/v1/apps", body).await,
        Some(id) => {
            client::request(reqwest::Method::PUT, &format!("/api/v1/apps/{id}"), body).await
        }
    }
}

/// `DELETE /api/v1/apps/{id}` — its service account stays.
pub async fn delete(id: &str) -> Result<(), ApiError> {
    client::request_opt::<Value>(reqwest::Method::DELETE, &format!("/api/v1/apps/{id}"), None)
        .await
        .map(|_| ())
}
