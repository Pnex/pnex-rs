//! AI assistant endpoints — status, LLM providers of the org (key = vault
//! reference, secrets.md D116), provider test, and the user's private
//! conversations (D145).

use pnex_core::{
    AiConversation, AiConversationDetail, AiConversationWrite, AiRetention, AiSendMessage,
    AiStatus, AiTurnResponse, LlmProvider, LlmProviderInput, LlmProviderTest, Paginated,
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

/// Conversations of the current user in the current org (D145).
const CONVERSATIONS: &str = "/api/v1/ai/conversations";

/// `GET …/conversations` — most recent first.
pub async fn conversations() -> Result<Paginated<AiConversation>, ApiError> {
    client::request(
        reqwest::Method::GET,
        &format!("{CONVERSATIONS}?limit=100"),
        None,
    )
    .await
}

/// `POST …/conversations` — new empty conversation.
pub async fn create_conversation() -> Result<AiConversation, ApiError> {
    let body = serde_json::to_value(AiConversationWrite::default()).unwrap_or_default();
    client::request(reqwest::Method::POST, CONVERSATIONS, Some(body)).await
}

/// `GET …/conversations/{id}` — every stored message (resume).
pub async fn conversation(id: Uuid) -> Result<AiConversationDetail, ApiError> {
    client::request(reqwest::Method::GET, &format!("{CONVERSATIONS}/{id}"), None).await
}

/// `PATCH …/conversations/{id}` — rename.
pub async fn rename_conversation(id: Uuid, title: String) -> Result<AiConversation, ApiError> {
    let body = serde_json::to_value(AiConversationWrite { title: Some(title) }).unwrap_or_default();
    client::request(
        reqwest::Method::PATCH,
        &format!("{CONVERSATIONS}/{id}"),
        Some(body),
    )
    .await
}

/// `DELETE …/conversations/{id}` — 204, permanent.
pub async fn delete_conversation(id: Uuid) -> Result<(), ApiError> {
    client::request_opt::<()>(
        reqwest::Method::DELETE,
        &format!("{CONVERSATIONS}/{id}"),
        None,
    )
    .await?;
    Ok(())
}

/// `DELETE …/conversations` — every conversation of mine in this org.
pub async fn delete_all_conversations() -> Result<(), ApiError> {
    client::request_opt::<serde_json::Value>(reqwest::Method::DELETE, CONVERSATIONS, None).await?;
    Ok(())
}

/// `GET …/conversations/export` — raw JSON export (portability).
pub async fn export_conversations() -> Result<serde_json::Value, ApiError> {
    client::request(
        reqwest::Method::GET,
        &format!("{CONVERSATIONS}/export"),
        None,
    )
    .await
}

/// `POST …/conversations/{id}/messages` — one agent turn; only the new
/// message travels, the server rebuilds the history.
pub async fn send_message(id: Uuid, msg: AiSendMessage) -> Result<AiTurnResponse, ApiError> {
    client::request(
        reqwest::Method::POST,
        &format!("{CONVERSATIONS}/{id}/messages"),
        Some(serde_json::to_value(msg).unwrap_or_default()),
    )
    .await
}

/// `GET /api/v1/ai/retention` — conversation retention of the org.
pub async fn retention() -> Result<AiRetention, ApiError> {
    client::request(reqwest::Method::GET, "/api/v1/ai/retention", None).await
}

/// `PUT /api/v1/ai/retention` — org value (owner/admin), `None` = follow
/// the platform.
pub async fn set_retention(days: Option<u32>) -> Result<AiRetention, ApiError> {
    client::request(
        reqwest::Method::PUT,
        "/api/v1/ai/retention",
        Some(serde_json::json!({ "days": days })),
    )
    .await
}

/// `PUT /api/v1/ai/retention/default` — platform value (platform admin),
/// `None` = built-in default.
pub async fn set_platform_retention(days: Option<u32>) -> Result<serde_json::Value, ApiError> {
    client::request(
        reqwest::Method::PUT,
        "/api/v1/ai/retention/default",
        Some(serde_json::json!({ "days": days })),
    )
    .await
}
