//! Mailboxes of the org (mail.md D453).

use pnex_core::mail::{MailAttachment, MailMessage, Mailbox, MailboxInput, MailboxTestResult};
use pnex_core::Paginated;

use crate::api::client;
use crate::api::error::ApiError;

/// `GET /api/v1/mailboxes`, every page.
pub async fn list() -> Result<Vec<Mailbox>, ApiError> {
    client::request_all("/api/v1/mailboxes").await
}

/// `POST /api/v1/mailboxes`.
pub async fn create(input: &MailboxInput) -> Result<Mailbox, ApiError> {
    client::request(
        reqwest::Method::POST,
        "/api/v1/mailboxes",
        Some(serde_json::to_value(input).unwrap_or_default()),
    )
    .await
}

/// `PUT /api/v1/mailboxes/{id}`: a password left out keeps the stored one.
pub async fn update(id: &str, input: &MailboxInput) -> Result<Mailbox, ApiError> {
    client::request(
        reqwest::Method::PUT,
        &format!("/api/v1/mailboxes/{id}"),
        Some(serde_json::to_value(input).unwrap_or_default()),
    )
    .await
}

/// `POST /api/v1/mailboxes/{id}/state`: resuming clears the last error.
pub async fn set_paused(id: &str, paused: bool) -> Result<Mailbox, ApiError> {
    client::request(
        reqwest::Method::POST,
        &format!("/api/v1/mailboxes/{id}/state"),
        Some(serde_json::json!({ "paused": paused })),
    )
    .await
}

/// `POST /api/v1/mailboxes/{id}/test`: connects and counts, fetches nothing.
pub async fn test(id: &str) -> Result<MailboxTestResult, ApiError> {
    client::request(
        reqwest::Method::POST,
        &format!("/api/v1/mailboxes/{id}/test"),
        None,
    )
    .await
}

/// `DELETE /api/v1/mailboxes/{id}`: its stored mails go with it.
pub async fn delete(id: &str) -> Result<(), ApiError> {
    client::request_opt::<()>(
        reqwest::Method::DELETE,
        &format!("/api/v1/mailboxes/{id}"),
        None,
    )
    .await?;
    Ok(())
}

/// `GET /api/v1/mailboxes/{id}/messages`, newest first; 404 when the
/// caller does not read this mailbox (D454).
pub async fn messages(
    id: &str,
    limit: i64,
    offset: i64,
) -> Result<Paginated<MailMessage>, ApiError> {
    client::request(
        reqwest::Method::GET,
        &format!("/api/v1/mailboxes/{id}/messages?limit={limit}&offset={offset}"),
        None,
    )
    .await
}

/// `POST /api/v1/mailboxes/{id}/messages/{message}/attachments/{index}/media`:
/// the server fetches the mail again and saves the attachment as a media.
pub async fn save_attachment(
    id: &str,
    message: &str,
    index: usize,
) -> Result<MailAttachment, ApiError> {
    client::request(
        reqwest::Method::POST,
        &format!("/api/v1/mailboxes/{id}/messages/{message}/attachments/{index}/media"),
        None,
    )
    .await
}

/// `POST /api/v1/mailboxes/{id}/messages/{message}/invitation/calendar`:
/// the meeting goes to the org calendar (or moves, or is cancelled).
pub async fn add_invitation(
    id: &str,
    message: &str,
) -> Result<pnex_core::time_range::TimeRange, ApiError> {
    client::request(
        reqwest::Method::POST,
        &format!("/api/v1/mailboxes/{id}/messages/{message}/invitation/calendar"),
        None,
    )
    .await
}
