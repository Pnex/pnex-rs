//! API client for the PNeX server hosts registry (`pnex_core::edge`) —
//! device wizard.

use crate::api::client;
use crate::api::error::ApiError;
use pnex_core::{PnexHost, PnexHostInput};

/// `GET /api/v1/edge/hosts` — every host of the org (all pages).
pub async fn list() -> Result<Vec<PnexHost>, ApiError> {
    client::request_all("/api/v1/edge/hosts").await
}

/// `POST /api/v1/edge/hosts` — upsert (200 or 201, same handling).
pub async fn create(input: PnexHostInput) -> Result<PnexHost, ApiError> {
    client::request(
        reqwest::Method::POST,
        "/api/v1/edge/hosts",
        Some(serde_json::to_value(input).unwrap_or_default()),
    )
    .await
}

/// `PUT /api/v1/edge/hosts/{id}` — in-place edit (id kept):
/// host rename (always wss since D70). 409 `edge-host-taken` when the target
/// host is already used by another entry of the org.
pub async fn update(id: i64, input: PnexHostInput) -> Result<PnexHost, ApiError> {
    client::request(
        reqwest::Method::PUT,
        &format!("/api/v1/edge/hosts/{id}"),
        Some(serde_json::to_value(input).unwrap_or_default()),
    )
    .await
}

/// `GET /api/v1/edge/lan-scan` — server-side LAN scan: meta probes over the
/// private /24 ranges (or an explicit RFC 1918 `a.b.c.` prefix, else 400
/// `field-invalid`) returning the servers identifying as `pnex-server`.
/// Owner, admin or member (viewer → 403).
pub async fn lan_scan(prefix: Option<&str>) -> Result<pnex_core::LanScanResult, ApiError> {
    // An "a.b.c." prefix (digits + dots) is URL-safe as is;
    // everything else is rejected by the server guard.
    let suffix = prefix
        .map(|p| format!("?prefix={}", p.trim()))
        .unwrap_or_default();
    client::request(
        reqwest::Method::GET,
        &format!("/api/v1/edge/lan-scan{suffix}"),
        None,
    )
    .await
}

/// `DELETE /api/v1/edge/hosts/{id}` — 204 with no body (`request_opt`: a
/// plain `request` would raise "empty response").
pub async fn delete(id: i64) -> Result<(), ApiError> {
    client::request_opt::<()>(
        reqwest::Method::DELETE,
        &format!("/api/v1/edge/hosts/{id}"),
        None,
    )
    .await?;
    Ok(())
}

/// `GET /api/v1/edge/hosts/locked` — server host imposed by the deployment
/// (`PNEX_PROD_HOST`), `None` when users pick from the referential.
pub async fn locked() -> Result<Option<String>, ApiError> {
    let locked: pnex_core::LockedHost =
        client::request(reqwest::Method::GET, "/api/v1/edge/hosts/locked", None).await?;
    Ok(locked.host)
}
