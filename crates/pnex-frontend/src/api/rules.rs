//! Rules (analysis.md D829–D830).

use pnex_core::rules::{Rule, RuleInput, RuleReplay};
use reqwest::Method;

use super::client;
use super::error::ApiError;

pub async fn list() -> Result<Vec<Rule>, ApiError> {
    client::request_all("/api/v1/rules").await
}

pub async fn save(id: Option<&str>, input: &RuleInput) -> Result<Rule, ApiError> {
    let body = Some(serde_json::to_value(input).unwrap_or_default());
    match id {
        Some(id) => client::request(Method::PUT, &format!("/api/v1/rules/{id}"), body).await,
        None => client::request(Method::POST, "/api/v1/rules", body).await,
    }
}

pub async fn delete(id: &str) -> Result<(), ApiError> {
    client::request_opt::<serde_json::Value>(Method::DELETE, &format!("/api/v1/rules/{id}"), None)
        .await
        .map(|_| ())
}

pub async fn replay(id: &str, window: &str) -> Result<RuleReplay, ApiError> {
    let body = Some(serde_json::json!({ "window": window }));
    client::request(Method::POST, &format!("/api/v1/rules/{id}/replay"), body).await
}
