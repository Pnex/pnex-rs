//! Endpoints organisations — CRUD + membres (multi-tenant Phase 3).

use pnex_core::{
    AddMember, CreateOrg, OrgDetail, OrgMember, OrgSummary, Paginated, UpdateMember, UpdateOrg,
};

use crate::api::client;
use crate::api::error::ApiError;

/// `GET /api/v1/orgs` — orgs dont je suis membre, enveloppe paginée (D14).
pub async fn list() -> Result<Paginated<OrgSummary>, ApiError> {
    client::request(reqwest::Method::GET, "/api/v1/orgs", None).await
}

/// `POST /api/v1/orgs` — création (créateur owner, tier Free).
pub async fn create(name: &str) -> Result<OrgSummary, ApiError> {
    client::request(
        reqwest::Method::POST,
        "/api/v1/orgs",
        Some(serde_json::to_value(CreateOrg { name: name.into() }).unwrap_or_default()),
    )
    .await
}

/// `GET /api/v1/orgs/{id}` — détail avec membres.
pub async fn detail(id: i64) -> Result<OrgDetail, ApiError> {
    client::request(reqwest::Method::GET, &format!("/api/v1/orgs/{id}"), None).await
}

/// `PATCH /api/v1/orgs/{id}` — renommage (owner/admin).
pub async fn rename(id: i64, name: &str) -> Result<(), ApiError> {
    client::request(
        reqwest::Method::PATCH,
        &format!("/api/v1/orgs/{id}"),
        Some(serde_json::to_value(UpdateOrg { name: name.into() }).unwrap_or_default()),
    )
    .await
}

/// `DELETE /api/v1/orgs/{id}` — suppression (owner et dernier membre).
pub async fn delete(id: i64) -> Result<(), ApiError> {
    client::request_opt::<serde_json::Value>(
        reqwest::Method::DELETE,
        &format!("/api/v1/orgs/{id}"),
        None,
    )
    .await
    .map(|_| ())
}

/// `POST /api/v1/orgs/{id}/members` — ajout par email d'un user déjà
/// provisionné (il s'est connecté au moins une fois).
pub async fn add_member(id: i64, email: &str, role: &str) -> Result<OrgMember, ApiError> {
    client::request(
        reqwest::Method::POST,
        &format!("/api/v1/orgs/{id}/members"),
        Some(
            serde_json::to_value(AddMember {
                email: email.into(),
                role: Some(role.into()),
            })
            .unwrap_or_default(),
        ),
    )
    .await
}

/// `PATCH /api/v1/orgs/{id}/members/{user_id}` — changement de rôle.
pub async fn update_member(id: i64, user_id: i64, role: &str) -> Result<(), ApiError> {
    client::request(
        reqwest::Method::PATCH,
        &format!("/api/v1/orgs/{id}/members/{user_id}"),
        Some(serde_json::to_value(UpdateMember { role: role.into() }).unwrap_or_default()),
    )
    .await
}

/// `DELETE /api/v1/orgs/{id}/members/{user_id}` — retrait / départ volontaire.
pub async fn remove_member(id: i64, user_id: i64) -> Result<(), ApiError> {
    client::request_opt::<serde_json::Value>(
        reqwest::Method::DELETE,
        &format!("/api/v1/orgs/{id}/members/{user_id}"),
        None,
    )
    .await
    .map(|_| ())
}
