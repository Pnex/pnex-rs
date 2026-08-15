//! Endpoints utilisateur — `user-info` (et plus tard `PATCH /api/v1/profile`,
//! branché avec la page Profil).

use pnex_core::UserInfo;

use crate::api::client;
use crate::api::error::ApiError;

/// `GET /api/v1/user-info` — identité + profil + orgs + comptage devices.
/// Déclenche le JIT provisioning côté backend à la première requête.
pub async fn get_user_info() -> Result<UserInfo, ApiError> {
    client::request(
        reqwest::Method::GET,
        "/api/v1/user-info",
        None,
    )
    .await
}
