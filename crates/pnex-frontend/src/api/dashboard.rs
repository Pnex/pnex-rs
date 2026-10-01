//! Endpoint dashboard — summary org-scope (2026-08-19).

use pnex_core::DashboardSummary;

use crate::api::client;
use crate::api::error::ApiError;

/// `GET /api/v1/dashboard/summary` — liveness + builds + dernières
/// mesures de l'org courante. La dégradation télémétrie est modélisée
/// dans le payload (`telemetry.available: false`), jamais en erreur HTTP.
pub async fn summary() -> Result<DashboardSummary, ApiError> {
    client::request(reqwest::Method::GET, "/api/v1/dashboard/summary", None).await
}
