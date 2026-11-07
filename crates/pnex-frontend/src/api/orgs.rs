//! Organization endpoints — CRUD, members and invitations (multi-tenant
//! Phase 3). The `/orgs/{id}` routes carry `X-Org-Id: {id}` (SEC-75): the
//! detail page may show an org that is not the current one.

use pnex_core::{
    CreateOrg, InviteMember, MyInvitation, OrgDetail, OrgInvitation, OrgRole, OrgSummary,
    UpdateMember, UpdateOrg,
};
use reqwest::Method;

use crate::api::client;
use crate::api::error::ApiError;

/// `GET /api/v1/orgs` — every org I am a member of (all pages).
pub async fn list() -> Result<Vec<OrgSummary>, ApiError> {
    client::request_all("/api/v1/orgs").await
}

/// `POST /api/v1/orgs` — creation (creator is owner, Free tier).
pub async fn create(name: &str) -> Result<OrgSummary, ApiError> {
    client::request(
        Method::POST,
        "/api/v1/orgs",
        Some(serde_json::to_value(CreateOrg { name: name.into() }).unwrap_or_default()),
    )
    .await
}

/// `GET /api/v1/orgs/{id}` — detail with members.
pub async fn detail(id: i64) -> Result<OrgDetail, ApiError> {
    client::request_in(id, Method::GET, &format!("/api/v1/orgs/{id}"), None).await
}

/// `PATCH /api/v1/orgs/{id}` — rename (owner/admin).
pub async fn rename(id: i64, name: &str) -> Result<(), ApiError> {
    client::request_in(
        id,
        Method::PATCH,
        &format!("/api/v1/orgs/{id}"),
        Some(serde_json::to_value(UpdateOrg { name: name.into() }).unwrap_or_default()),
    )
    .await
    .map(|_: serde_json::Value| ())
}

/// `DELETE /api/v1/orgs/{id}` — deletion (owner and last member).
pub async fn delete(id: i64) -> Result<(), ApiError> {
    client::request_in(id, Method::DELETE, &format!("/api/v1/orgs/{id}"), None).await
}

/// `POST /api/v1/orgs/{id}/invitations` — invites an email; nobody joins
/// before accepting.
pub async fn invite(id: i64, email: &str, role: OrgRole) -> Result<OrgInvitation, ApiError> {
    client::request_in(
        id,
        Method::POST,
        &format!("/api/v1/orgs/{id}/invitations"),
        Some(
            serde_json::to_value(InviteMember {
                email: email.into(),
                role: Some(role),
            })
            .unwrap_or_default(),
        ),
    )
    .await
}

/// `GET /api/v1/orgs/{id}/invitations` — pending invitations (owner/admin).
pub async fn invitations(id: i64) -> Result<Vec<OrgInvitation>, ApiError> {
    // shortcut: first 100 only, page through them if an org ever keeps more pending.
    let page: pnex_core::Paginated<OrgInvitation> = client::request_in(
        id,
        Method::GET,
        &format!("/api/v1/orgs/{id}/invitations?limit=100"),
        None,
    )
    .await?;
    Ok(page.results)
}

/// `DELETE /api/v1/orgs/{id}/invitations/{invitation_id}` — revocation.
pub async fn revoke_invitation(id: i64, invitation_id: i64) -> Result<(), ApiError> {
    client::request_in(
        id,
        Method::DELETE,
        &format!("/api/v1/orgs/{id}/invitations/{invitation_id}"),
        None,
    )
    .await
}

/// `GET /api/v1/invitations` — invitations waiting for my answer.
pub async fn my_invitations() -> Result<Vec<MyInvitation>, ApiError> {
    client::request_all("/api/v1/invitations").await
}

/// `POST /api/v1/invitations/{id}/accept` — joins the org.
pub async fn accept_invitation(id: i64) -> Result<OrgSummary, ApiError> {
    client::request(
        Method::POST,
        &format!("/api/v1/invitations/{id}/accept"),
        None,
    )
    .await
}

/// `POST /api/v1/invitations/{id}/decline` — refuses it.
pub async fn decline_invitation(id: i64) -> Result<(), ApiError> {
    client::request_opt::<serde_json::Value>(
        Method::POST,
        &format!("/api/v1/invitations/{id}/decline"),
        None,
    )
    .await
    .map(|_| ())
}

/// `PATCH /api/v1/orgs/{id}/members/{user_id}` — role change.
pub async fn update_member(id: i64, user_id: i64, role: OrgRole) -> Result<(), ApiError> {
    client::request_in(
        id,
        Method::PATCH,
        &format!("/api/v1/orgs/{id}/members/{user_id}"),
        Some(serde_json::to_value(UpdateMember { role }).unwrap_or_default()),
    )
    .await
    .map(|_: serde_json::Value| ())
}

/// `DELETE /api/v1/orgs/{id}/members/{user_id}` — removal.
pub async fn remove_member(id: i64, user_id: i64) -> Result<(), ApiError> {
    client::request_in(
        id,
        Method::DELETE,
        &format!("/api/v1/orgs/{id}/members/{user_id}"),
        None,
    )
    .await
}
