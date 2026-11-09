//! MT5 terminals as registered data sources (markets-research.md D847–D849).

use pnex_core::mt5::{Mt5Backfill, Mt5Symbol, Mt5Terminal, Mt5TerminalInput};
use pnex_core::Paginated;
use reqwest::Method;

use super::client;
use super::error::ApiError;
use crate::api::media::urlencode;

pub async fn list(limit: i64, offset: i64) -> Result<Paginated<Mt5Terminal>, ApiError> {
    client::request(
        Method::GET,
        &format!("/api/v1/mt5/terminals?limit={limit}&offset={offset}"),
        None,
    )
    .await
}

pub async fn get(id: &str) -> Result<Mt5Terminal, ApiError> {
    client::request(Method::GET, &format!("/api/v1/mt5/terminals/{id}"), None).await
}

/// Creates the terminal, or replaces it on the version read (`input.expected_version`).
pub async fn save(id: Option<&str>, input: &Mt5TerminalInput) -> Result<Mt5Terminal, ApiError> {
    let body = Some(serde_json::to_value(input).unwrap_or_default());
    match id {
        Some(id) => {
            client::request(Method::PUT, &format!("/api/v1/mt5/terminals/{id}"), body).await
        }
        None => client::request(Method::POST, "/api/v1/mt5/terminals", body).await,
    }
}

pub async fn delete(id: &str) -> Result<(), ApiError> {
    client::request_opt::<serde_json::Value>(
        Method::DELETE,
        &format!("/api/v1/mt5/terminals/{id}"),
        None,
    )
    .await
    .map(|_| ())
}

/// The terminal's Expert Advisor, with a new token: the previous one stops.
pub async fn expert_advisor(id: &str) -> Result<Vec<u8>, ApiError> {
    client::request_bytes(Method::POST, &format!("/api/v1/mt5/terminals/{id}/ea")).await
}

pub async fn backfill(id: &str, days: u32) -> Result<Mt5Terminal, ApiError> {
    let body = Some(serde_json::to_value(Mt5Backfill { days }).unwrap_or_default());
    client::request(
        Method::POST,
        &format!("/api/v1/mt5/terminals/{id}/backfill"),
        body,
    )
    .await
}

pub async fn symbols(
    id: &str,
    q: &str,
    limit: i64,
    offset: i64,
) -> Result<Paginated<Mt5Symbol>, ApiError> {
    let mut path = format!("/api/v1/mt5/terminals/{id}/symbols?limit={limit}&offset={offset}");
    if !q.trim().is_empty() {
        path.push_str(&format!("&q={}", urlencode(q.trim())));
    }
    client::request(Method::GET, &path, None).await
}
