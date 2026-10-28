//! Dashboard endpoint — org-scoped summary (2026-08-19).

use pnex_core::DashboardSummary;

use crate::api::client;
use crate::api::error::ApiError;

/// `GET /api/v1/dashboard/summary` — liveness + builds + latest
/// readings of the current org. Telemetry degradation is modelled
/// in the payload (`telemetry.available: false`), never as an HTTP error.
pub async fn summary() -> Result<DashboardSummary, ApiError> {
    client::request(reqwest::Method::GET, "/api/v1/dashboard/summary", None).await
}
