//! Time ranges API (media-ingest.md D169): `/api/v1/time-ranges` (D14
//! list by scope and window, create, update, delete, CSV/ICS import).

use pnex_core::time_range::{RangeImportResult, TimeRange, TimeRangeInput};
use pnex_core::Paginated;

use crate::api::client;
use crate::api::error::ApiError;
use crate::api::media::urlencode;

/// `GET /api/v1/time-ranges` — ranges of a scope meeting `[from, to)`.
pub async fn list(
    scope_kind: &str,
    scope_id: &str,
    from: &str,
    to: &str,
    limit: i64,
    offset: i64,
) -> Result<Paginated<TimeRange>, ApiError> {
    let pairs = [
        format!("scope_kind={}", urlencode(scope_kind)),
        format!("scope_id={}", urlencode(scope_id)),
        format!("from={}", urlencode(from)),
        format!("to={}", urlencode(to)),
        format!("limit={limit}"),
        format!("offset={offset}"),
    ];
    client::request(
        reqwest::Method::GET,
        &format!("/api/v1/time-ranges?{}", pairs.join("&")),
        None,
    )
    .await
}

/// `POST /api/v1/time-ranges` — 400 body = field tokens.
pub async fn create(input: &TimeRangeInput) -> Result<TimeRange, ApiError> {
    client::request(
        reqwest::Method::POST,
        "/api/v1/time-ranges",
        Some(serde_json::to_value(input).unwrap_or_default()),
    )
    .await
}

/// `PATCH /api/v1/time-ranges/{id}` — absent fields kept, empty cleared.
pub async fn update(id: &str, input: &TimeRangeInput) -> Result<TimeRange, ApiError> {
    client::request(
        reqwest::Method::PATCH,
        &format!("/api/v1/time-ranges/{id}"),
        Some(serde_json::to_value(input).unwrap_or_default()),
    )
    .await
}

/// `DELETE /api/v1/time-ranges/{id}`.
pub async fn delete(id: &str) -> Result<Option<()>, ApiError> {
    client::request_opt(
        reqwest::Method::DELETE,
        &format!("/api/v1/time-ranges/{id}"),
        None,
    )
    .await
}

/// `POST /api/v1/time-ranges/import` — raw CSV or ICS body.
pub async fn import(
    format: &str,
    scope_kind: &str,
    scope_id: &str,
    bytes: Vec<u8>,
) -> Result<RangeImportResult, ApiError> {
    client::request_upload(
        reqwest::Method::POST,
        &format!(
            "/api/v1/time-ranges/import?format={}&scope_kind={}&scope_id={}",
            urlencode(format),
            urlencode(scope_kind),
            urlencode(scope_id)
        ),
        bytes,
    )
    .await
}
