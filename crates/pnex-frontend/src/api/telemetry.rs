//! Telemetry endpoints — catalogue of the available series (dashboard,
//! flow and annotation pickers).

use pnex_core::TelemetryCatalog;

use crate::api::client;
use crate::api::error::ApiError;

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
