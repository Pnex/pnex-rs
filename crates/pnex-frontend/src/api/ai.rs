//! AI assistant endpoints — status, LLM providers of the org (key = vault
//! reference, secrets.md D116), provider test, chat.

use pnex_core::{
    AiChatRequest, AiChatResponse, AiStatus, LlmProvider, LlmProviderInput, LlmProviderTest,
};
use uuid::Uuid;

use crate::api::client;
use crate::api::error::ApiError;

/// `GET /api/v1/ai/status`.
pub async fn status() -> Result<AiStatus, ApiError> {
    client::request(reqwest::Method::GET, "/api/v1/ai/status", None).await
}

/// Providers of the current org.
const PROVIDERS: &str = "/api/v1/ai/providers";

/// `GET …/providers`.
pub async fn providers() -> Result<Vec<LlmProvider>, ApiError> {
    client::request(reqwest::Method::GET, PROVIDERS, None).await
}

/// `POST …/providers` — the key is required.
pub async fn create_provider(input: &LlmProviderInput) -> Result<LlmProvider, ApiError> {
    let body = serde_json::to_value(input).map_err(|e| ApiError::new(e.to_string()))?;
    client::request(reqwest::Method::POST, PROVIDERS, Some(body)).await
}

/// `PUT …/providers/{id}` — `api_key: None` keeps the current key.
pub async fn update_provider(id: Uuid, input: &LlmProviderInput) -> Result<LlmProvider, ApiError> {
    let body = serde_json::to_value(input).map_err(|e| ApiError::new(e.to_string()))?;
    client::request(
        reqwest::Method::PUT,
        &format!("{PROVIDERS}/{id}"),
        Some(body),
    )
    .await
}

/// `DELETE …/providers/{id}` — 204.
pub async fn delete_provider(id: Uuid) -> Result<(), ApiError> {
    client::request_opt::<()>(reqwest::Method::DELETE, &format!("{PROVIDERS}/{id}"), None).await?;
    Ok(())
}

/// `POST …/providers/{id}/test` — one real LLM request.
pub async fn test_provider(id: Uuid) -> Result<LlmProviderTest, ApiError> {
    client::request(
        reqwest::Method::POST,
        &format!("{PROVIDERS}/{id}/test"),
        None,
    )
    .await
}

/// `POST /api/v1/ai/chat` — un tour complet (boucle d'outils côté serveur).
pub async fn chat(req: AiChatRequest) -> Result<AiChatResponse, ApiError> {
    client::request(
        reqwest::Method::POST,
        "/api/v1/ai/chat",
        Some(serde_json::to_value(req).unwrap_or_default()),
    )
    .await
}
