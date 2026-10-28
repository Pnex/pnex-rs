//! User endpoints — `user-info` and `PATCH /api/v1/profile`.

use pnex_core::{ProfilePatch, UserInfo, UserProfile};

use crate::api::client;
use crate::api::error::ApiError;

/// `GET /api/v1/user-info` — identity + profile + orgs + device count.
/// Triggers JIT provisioning on the backend on the first request.
pub async fn get_user_info() -> Result<UserInfo, ApiError> {
    client::request(reqwest::Method::GET, "/api/v1/user-info", None).await
}

/// `PATCH /api/v1/profile` — preferences (provided fields only).
pub async fn patch_profile(patch: &ProfilePatch) -> Result<UserProfile, ApiError> {
    client::request(
        reqwest::Method::PATCH,
        "/api/v1/profile",
        Some(serde_json::to_value(patch).unwrap_or_default()),
    )
    .await
}
