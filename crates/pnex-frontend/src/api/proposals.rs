//! Proposals waiting for a human (proposals.md D374).

use pnex_core::ontology::proposal::{DecisionKind, ProposalDecision, ProposalView};
use pnex_core::Paginated;

use crate::api::client;
use crate::api::error::ApiError;
use crate::components::crud::pager::PAGE_SIZE;

/// `GET /api/v1/proposals`: one page, by status, optionally about an object.
pub async fn page(
    status: &str,
    object_id: Option<&str>,
    page: i64,
) -> Result<Paginated<ProposalView>, ApiError> {
    let object = object_id
        .map(|o| format!("&object_id={o}"))
        .unwrap_or_default();
    client::request(
        reqwest::Method::GET,
        &format!(
            "/api/v1/proposals?status={status}&limit={PAGE_SIZE}&offset={}{object}",
            page.max(0) * PAGE_SIZE
        ),
        None,
    )
    .await
}

/// `POST /api/v1/proposals/{id}/decision`.
pub async fn decide(id: &str, decision: DecisionKind) -> Result<ProposalView, ApiError> {
    let body = ProposalDecision {
        decision,
        reason: None,
        values: None,
    };
    client::request(
        reqwest::Method::POST,
        &format!("/api/v1/proposals/{id}/decision"),
        Some(serde_json::to_value(body).unwrap_or_default()),
    )
    .await
}
