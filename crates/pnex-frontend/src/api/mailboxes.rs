//! Mailboxes of the org (mail.md D453).

use pnex_core::mail::Mailbox;

use crate::api::client;
use crate::api::error::ApiError;

/// `GET /api/v1/mailboxes`, every page.
pub async fn list() -> Result<Vec<Mailbox>, ApiError> {
    client::request_all("/api/v1/mailboxes").await
}
