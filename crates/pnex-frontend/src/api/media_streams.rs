//! Media ingest API (media-ingest.md P2.13) — client of
//! `/api/v1/media/streams` (D14 list, create, update, delete)
//! and `/api/v1/asr/profiles`.

use pnex_core::media_ingest::{AsrProfile, MediaStream, MediaStreamInput};
use pnex_core::Paginated;

use crate::api::client;
use crate::api::error::ApiError;

fn page_query(limit: Option<i64>, offset: Option<i64>) -> String {
    let mut pairs = Vec::new();
    if let Some(limit) = limit {
        pairs.push(format!("limit={limit}"));
    }
    if let Some(offset) = offset {
        pairs.push(format!("offset={offset}"));
    }
    if pairs.is_empty() {
        String::new()
    } else {
        format!("?{}", pairs.join("&"))
    }
}

/// `GET /api/v1/media/streams` — D14 envelope.
pub async fn list(
    limit: Option<i64>,
    offset: Option<i64>,
) -> Result<Paginated<MediaStream>, ApiError> {
    client::request(
        reqwest::Method::GET,
        &format!("/api/v1/media/streams{}", page_query(limit, offset)),
        None,
    )
    .await
}

/// `POST /api/v1/media/streams` — 400 body = field tokens.
pub async fn create(input: &MediaStreamInput) -> Result<MediaStream, ApiError> {
    client::request(
        reqwest::Method::POST,
        "/api/v1/media/streams",
        Some(serde_json::to_value(input).unwrap_or_default()),
    )
    .await
}

/// `PATCH /api/v1/media/streams/{id}` — absent fields untouched.
pub async fn update(id: &str, input: &MediaStreamInput) -> Result<MediaStream, ApiError> {
    client::request(
        reqwest::Method::PATCH,
        &format!("/api/v1/media/streams/{id}"),
        Some(serde_json::to_value(input).unwrap_or_default()),
    )
    .await
}

/// `DELETE /api/v1/media/streams/{id}`.
pub async fn delete(id: &str) -> Result<Option<()>, ApiError> {
    client::request_opt(
        reqwest::Method::DELETE,
        &format!("/api/v1/media/streams/{id}"),
        None,
    )
    .await
}

/// `GET /api/v1/asr/profiles`.
pub async fn profiles() -> Result<Vec<AsrProfile>, ApiError> {
    client::request(reqwest::Method::GET, "/api/v1/asr/profiles", None).await
}
