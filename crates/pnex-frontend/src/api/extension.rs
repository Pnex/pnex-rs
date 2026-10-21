//! Browser agents API (extension-collector.md D254–D270):
//! `/api/v1/extension/{agents,missions,download}`.

use pnex_core::browser_agent::{
    BrowserAgent, BrowserAgentPatch, Mission, MissionCreate, MissionPatch,
};

use crate::api::client;
use crate::api::error::ApiError;

fn body<T: serde::Serialize>(input: &T) -> Option<serde_json::Value> {
    Some(serde_json::to_value(input).unwrap_or_default())
}

/// `GET /api/v1/extension/agents` — newest first, with `online`.
pub async fn agents() -> Result<Vec<BrowserAgent>, ApiError> {
    client::request_all("/api/v1/extension/agents").await
}

/// `PATCH /api/v1/extension/agents/{id}` — name and mode (D269).
pub async fn update_agent(id: i64, patch: &BrowserAgentPatch) -> Result<BrowserAgent, ApiError> {
    client::request(
        reqwest::Method::PATCH,
        &format!("/api/v1/extension/agents/{id}"),
        body(patch),
    )
    .await
}

/// `GET /api/v1/extension/missions` — newest first.
pub async fn missions() -> Result<Vec<Mission>, ApiError> {
    client::request_all("/api/v1/extension/missions").await
}

/// `POST /api/v1/extension/missions`.
pub async fn create_mission(input: &MissionCreate) -> Result<Mission, ApiError> {
    client::request(
        reqwest::Method::POST,
        "/api/v1/extension/missions",
        body(input),
    )
    .await
}

/// `PATCH /api/v1/extension/missions/{id}` — 409
/// `extension-mission-version-conflict` on a stale `expected_version`.
pub async fn update_mission(id: &str, patch: &MissionPatch) -> Result<Mission, ApiError> {
    client::request(
        reqwest::Method::PATCH,
        &format!("/api/v1/extension/missions/{id}"),
        body(patch),
    )
    .await
}

/// `DELETE /api/v1/extension/missions/{id}`.
pub async fn delete_mission(id: &str) -> Result<Option<()>, ApiError> {
    client::request_opt(
        reqwest::Method::DELETE,
        &format!("/api/v1/extension/missions/{id}"),
        None,
    )
    .await
}

/// `GET /api/v1/extension/videos` — one page of the video registry.
pub async fn videos(
    limit: i64,
    offset: i64,
) -> Result<pnex_core::Paginated<pnex_core::browser_agent::SocialVideo>, ApiError> {
    client::request(
        reqwest::Method::GET,
        &format!("/api/v1/extension/videos?limit={limit}&offset={offset}"),
        None,
    )
    .await
}

/// Public download of the extension zip (server address preset).
pub fn download_url() -> String {
    format!(
        "{}/api/v1/extension/download",
        crate::api::config::api_base()
    )
}

/// `GET /api/v1/extension/missions/{id}/pages` — the site registry (D309).
pub async fn site_pages(
    mission_id: &str,
) -> Result<Vec<pnex_core::browser_agent::SitePage>, ApiError> {
    client::request_all(&format!("/api/v1/extension/missions/{mission_id}/pages")).await
}

/// `GET /api/v1/extension/proofs?page_id=` — proofs of a page (D310).
pub async fn page_proofs(
    page_id: &str,
) -> Result<Vec<pnex_core::browser_agent::WebProof>, ApiError> {
    client::request_all(&format!("/api/v1/extension/proofs?page_id={page_id}")).await
}

/// `GET /api/v1/extension/proofs/verify` — the org's chain recomputed.
pub async fn verify_proofs() -> Result<pnex_core::browser_agent::ProofChainCheck, ApiError> {
    client::request(
        reqwest::Method::GET,
        "/api/v1/extension/proofs/verify",
        None,
    )
    .await
}

/// `POST /api/v1/extension/people/{id}/profile-captures` (D333).
pub async fn capture_profile(person_id: &str, url: &str) -> Result<Mission, ApiError> {
    client::request(
        reqwest::Method::POST,
        &format!("/api/v1/extension/people/{person_id}/profile-captures"),
        body(&pnex_core::browser_agent::ProfileCaptureRequest { url: url.into() }),
    )
    .await
}

/// Whether a person objected to prospecting (D341).
#[derive(Debug, Clone, PartialEq, serde::Deserialize)]
pub struct Opposition {
    pub opposed: bool,
}

/// `GET /api/v1/extension/people/{id}/opposition` (D341).
pub async fn opposition(person_id: &str) -> Result<Opposition, ApiError> {
    client::request(
        reqwest::Method::GET,
        &format!("/api/v1/extension/people/{person_id}/opposition"),
        None,
    )
    .await
}

/// `POST /api/v1/extension/people/{id}/opposition` (D341).
pub async fn oppose(person_id: &str) -> Result<Opposition, ApiError> {
    client::request(
        reqwest::Method::POST,
        &format!("/api/v1/extension/people/{person_id}/opposition"),
        None,
    )
    .await
}
