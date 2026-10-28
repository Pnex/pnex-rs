//! Telemetry endpoints (Visualisation page, 2026-08-19) — catalogue
//! of available series and points of a series over a preset window.

use pnex_core::{TelemetryCatalog, TelemetrySeriesResponse};

use crate::api::client;
use crate::api::error::ApiError;

/// Minimal percent-encoding (the front does not have the url crate).
fn urlencode(value: &str) -> String {
    let mut out = String::new();
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(byte as char);
            }
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

/// `GET /api/v1/telemetry/catalog` — series (metric x device) of the
/// current org. The 503 `telemetry-unavailable` maps back to an
/// `available: false` catalog: every picker already renders that state.
pub async fn catalog() -> Result<TelemetryCatalog, ApiError> {
    match client::request(reqwest::Method::GET, "/api/v1/telemetry/catalog", None).await {
        Err(e) if unavailable(&e) => Ok(TelemetryCatalog {
            available: false,
            series: Vec::new(),
            label_sets: Vec::new(),
        }),
        other => other,
    }
}

/// OpenObserve unavailable (503 `telemetry-unavailable`).
fn unavailable(e: &ApiError) -> bool {
    e.code.as_deref() == Some(pnex_core::err_codes::TELEMETRY_UNAVAILABLE)
}

/// `GET /api/v1/telemetry/series` — points of ONE series; `window` ∈
/// 1h/6h/24h (validated server-side, closed charset against injection).
pub async fn series(
    metric: &str,
    device_id: &str,
    window: &str,
) -> Result<TelemetrySeriesResponse, ApiError> {
    let res = client::request(
        reqwest::Method::GET,
        &format!(
            "/api/v1/telemetry/series?metric={}&device_id={}&window={window}",
            urlencode(metric),
            urlencode(device_id)
        ),
        None,
    )
    .await;
    match res {
        // Same mapping as the catalog: the page shows its "no data" state.
        Err(e) if unavailable(&e) => Ok(TelemetrySeriesResponse {
            available: false,
            metric: metric.to_string(),
            device_id: device_id.to_string(),
            points: Vec::new(),
        }),
        other => other,
    }
}
