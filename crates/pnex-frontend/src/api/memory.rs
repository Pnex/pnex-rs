//! Org shared memory (flow `memory-write` node): live key listing for the
//! pickers and numeric values for the dashboard widgets.

use pnex_core::memory::{MemoryKeyInfo, MemoryRef, MemoryValuesRequest, MemoryValuesResponse};
use pnex_core::TelemetryPoint;

use crate::api::client;
use crate::api::error::ApiError;

/// `GET /api/v1/memory/keys` — live keys of the current org (empty when
/// the memory store is disabled).
pub async fn keys() -> Result<Vec<MemoryKeyInfo>, ApiError> {
    client::request(reqwest::Method::GET, "/api/v1/memory/keys", None).await
}

/// `POST /api/v1/memory/values` — numeric values, same order as `refs`.
pub async fn values(refs: Vec<MemoryRef>) -> Result<MemoryValuesResponse, ApiError> {
    client::request(
        reqwest::Method::POST,
        "/api/v1/memory/values",
        Some(serde_json::to_value(MemoryValuesRequest { refs }).unwrap_or_default()),
    )
    .await
}

/// Resolves `refs` into live-values map entries (`series_key` → a single
/// point, `None` = unavailable), the shape the widgets already consume.
/// A failed call degrades every ref to `None`.
pub async fn live_entries(refs: Vec<MemoryRef>) -> Vec<(String, Option<Vec<TelemetryPoint>>)> {
    if refs.is_empty() {
        return Vec::new();
    }
    let keys: Vec<String> = refs.iter().map(MemoryRef::series_key).collect();
    let results = values(refs)
        .await
        .ok()
        .map(|r| r.results)
        .unwrap_or_default();
    keys.into_iter()
        .enumerate()
        .map(|(i, key)| {
            let point = results.get(i).and_then(|r| {
                Some(TelemetryPoint {
                    ts: r.ts_ms? as f64 / 1000.0,
                    value: r.value?,
                })
            });
            (key, point.map(|p| vec![p]))
        })
        .collect()
}
