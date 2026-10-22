//! Website candidates waiting for a human (web-presence.md D366).

use pnex_core::ontology::api::LinkView;
use pnex_core::web_visit::{SiteCandidate, SiteDecision, SiteDecisionKind};

use crate::api::client;
use crate::api::error::ApiError;

/// `GET /api/v1/web-presence/candidates`, every page.
pub async fn candidates() -> Result<Vec<SiteCandidate>, ApiError> {
    client::request_all("/api/v1/web-presence/candidates").await
}

/// `POST /api/v1/web-presence/candidates/{id}/decision`.
pub async fn decide(link_id: i64, decision: SiteDecisionKind) -> Result<LinkView, ApiError> {
    let body = serde_json::to_value(SiteDecision { decision }).unwrap_or_default();
    client::request(
        reqwest::Method::POST,
        &format!("/api/v1/web-presence/candidates/{link_id}/decision"),
        Some(body),
    )
    .await
}
