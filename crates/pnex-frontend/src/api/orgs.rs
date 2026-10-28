//! Organization endpoints — CRUD + members (multi-tenant Phase 3).

use pnex_core::{
    AddMember, CreateOrg, OrgDetail, OrgMember, OrgRole, OrgSummary, UpdateMember, UpdateOrg,
};

use crate::api::client;
use crate::api::error::ApiError;

/// `GET /api/v1/orgs` — every org I am a member of (all pages).
pub async fn list() -> Result<Vec<OrgSummary>, ApiError> {
    client::request_all("/api/v1/orgs").await
}

/// `POST /api/v1/orgs` — creation (creator is owner, Free tier).
pub async fn create(name: &str) -> Result<OrgSummary, ApiError> {
    client::request(
        reqwest::Method::POST,
        "/api/v1/orgs",
        Some(serde_json::to_value(CreateOrg { name: name.into() }).unwrap_or_default()),
    )
    .await
}

/// `GET /api/v1/orgs/{id}` — detail with members.
pub async fn detail(id: i64) -> Result<OrgDetail, ApiError> {
    client::request(reqwest::Method::GET, &format!("/api/v1/orgs/{id}"), None).await
}

/// `PATCH /api/v1/orgs/{id}` — rename (owner/admin).
pub async fn rename(id: i64, name: &str) -> Result<(), ApiError> {
    client::request(
        reqwest::Method::PATCH,
        &format!("/api/v1/orgs/{id}"),
        Some(serde_json::to_value(UpdateOrg { name: name.into() }).unwrap_or_default()),
    )
    .await
}

/// `DELETE /api/v1/orgs/{id}` — deletion (owner and last member).
pub async fn delete(id: i64) -> Result<(), ApiError> {
    client::request_opt::<serde_json::Value>(
        reqwest::Method::DELETE,
        &format!("/api/v1/orgs/{id}"),
        None,
    )
    .await
    .map(|_| ())
}

/// `POST /api/v1/orgs/{id}/members` — adds by email a user already
/// provisioned (who has logged in at least once).
pub async fn add_member(id: i64, email: &str, role: OrgRole) -> Result<OrgMember, ApiError> {
    client::request(
        reqwest::Method::POST,
        &format!("/api/v1/orgs/{id}/members"),
        Some(
            serde_json::to_value(AddMember {
                email: email.into(),
                role: Some(role),
            })
            .unwrap_or_default(),
        ),
    )
    .await
}

/// `PATCH /api/v1/orgs/{id}/members/{user_id}` — role change.
pub async fn update_member(id: i64, user_id: i64, role: OrgRole) -> Result<(), ApiError> {
    client::request(
        reqwest::Method::PATCH,
        &format!("/api/v1/orgs/{id}/members/{user_id}"),
        Some(serde_json::to_value(UpdateMember { role }).unwrap_or_default()),
    )
    .await
}

/// `DELETE /api/v1/orgs/{id}/members/{user_id}` — removal / voluntary leave.
pub async fn remove_member(id: i64, user_id: i64) -> Result<(), ApiError> {
    client::request_opt::<serde_json::Value>(
        reqwest::Method::DELETE,
        &format!("/api/v1/orgs/{id}/members/{user_id}"),
        None,
    )
    .await
    .map(|_| ())
}
