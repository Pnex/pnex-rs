//! Media ingest API (media-ingest.md P2.13) — client of
//! `/api/v1/media/streams` (D14 list, create, update, delete),
//! `/api/v1/media/transcripts`, `/api/v1/asr/models` and `/api/v1/asr/profiles`.

use pnex_core::media_ingest::{
    AsrModel, AsrModelInput, AsrProfile, AsrProfileInput, MediaSegment, MediaStream,
    MediaStreamInput, TranscriptRecord,
};
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

/// `POST /api/v1/asr/profiles`.
pub async fn create_profile(input: &AsrProfileInput) -> Result<AsrProfile, ApiError> {
    client::request(
        reqwest::Method::POST,
        "/api/v1/asr/profiles",
        Some(serde_json::to_value(input).unwrap_or_default()),
    )
    .await
}

/// `DELETE /api/v1/asr/profiles/{id}`.
pub async fn delete_profile(id: &str) -> Result<Option<()>, ApiError> {
    client::request_opt(
        reqwest::Method::DELETE,
        &format!("/api/v1/asr/profiles/{id}"),
        None,
    )
    .await
}

/// `GET /api/v1/asr/models`.
pub async fn models() -> Result<Vec<AsrModel>, ApiError> {
    client::request(reqwest::Method::GET, "/api/v1/asr/models", None).await
}

/// `POST /api/v1/asr/models` — reads the files and checks the model
/// before answering (can take a while for a large model).
pub async fn create_model(input: &AsrModelInput) -> Result<AsrModel, ApiError> {
    client::request(
        reqwest::Method::POST,
        "/api/v1/asr/models",
        Some(serde_json::to_value(input).unwrap_or_default()),
    )
    .await
}

/// `POST /api/v1/asr/models/{id}/check`.
pub async fn check_model(id: &str) -> Result<AsrModel, ApiError> {
    client::request(
        reqwest::Method::POST,
        &format!("/api/v1/asr/models/{id}/check"),
        None,
    )
    .await
}

/// `POST /api/v1/asr/models/{id}/test` — transcribes a dropped clip; the
/// file name gives the demuxer (the body goes as octet-stream).
pub async fn test_model(
    id: &str,
    file_name: &str,
    language: &str,
    bytes: Vec<u8>,
) -> Result<pnex_core::media_ingest::AsrTestResult, ApiError> {
    client::request_upload(
        reqwest::Method::POST,
        &format!(
            "/api/v1/asr/models/{id}/test?name={}&language={}",
            crate::api::media::urlencode(file_name),
            crate::api::media::urlencode(language)
        ),
        bytes,
    )
    .await
}

/// `DELETE /api/v1/asr/models/{id}`.
pub async fn delete_model(id: &str) -> Result<Option<()>, ApiError> {
    client::request_opt(
        reqwest::Method::DELETE,
        &format!("/api/v1/asr/models/{id}"),
        None,
    )
    .await
}

/// `GET /api/v1/media/transcripts` — newest first; `stream` empty = every
/// stream of the org.
pub async fn transcripts(
    stream: &str,
    q: &str,
    limit: i64,
    offset: i64,
) -> Result<Paginated<TranscriptRecord>, ApiError> {
    let mut pairs = vec![format!("limit={limit}"), format!("offset={offset}")];
    if !stream.is_empty() {
        pairs.push(format!("stream={}", crate::api::media::urlencode(stream)));
    }
    if !q.trim().is_empty() {
        pairs.push(format!("q={}", crate::api::media::urlencode(q.trim())));
    }
    client::request(
        reqwest::Method::GET,
        &format!("/api/v1/media/transcripts?{}", pairs.join("&")),
        None,
    )
    .await
}

/// `GET /api/v1/media/streams/{id}/segments` — newest first, optional
/// `state` filter (D14 envelope).
pub async fn segments(
    id: &str,
    state: &str,
    limit: i64,
    offset: i64,
) -> Result<Paginated<MediaSegment>, ApiError> {
    let mut pairs = vec![format!("limit={limit}"), format!("offset={offset}")];
    if !state.is_empty() {
        pairs.push(format!("state={state}"));
    }
    client::request(
        reqwest::Method::GET,
        &format!("/api/v1/media/streams/{id}/segments?{}", pairs.join("&")),
        None,
    )
    .await
}

/// `POST /api/v1/media/streams/{id}/segments/retry` — re-queues the failed
/// segments whose audio is still kept; returns how many.
pub async fn retry_segments(id: &str) -> Result<i64, ApiError> {
    let v: serde_json::Value = client::request(
        reqwest::Method::POST,
        &format!("/api/v1/media/streams/{id}/segments/retry"),
        None,
    )
    .await?;
    Ok(v["requeued"].as_i64().unwrap_or(0))
}
