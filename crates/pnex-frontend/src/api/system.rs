//! System endpoints (D72): O2 retention and cleanup of the current org,
//! platform status (platform admin). Shared `pnex-core` types.

use pnex_core::{
    O2DeleteRangeRequest, O2DeleteResult, O2PurgeRequest, O2StreamList, OrgSystemRow,
    OrgTierUpdate, RetentionInfo, RetentionUpdate, SystemStatus, TierOption,
};

use crate::api::client;
use crate::api::error::ApiError;

/// `GET /api/v1/system/retention` — effective retention of the current org.
pub async fn retention() -> Result<RetentionInfo, ApiError> {
    client::request(reqwest::Method::GET, "/api/v1/system/retention", None).await
}

/// `PUT /api/v1/system/retention/orgs/{id}` — platform admin; `None` clears.
pub async fn set_org_retention(org_id: i64, days: Option<u32>) -> Result<RetentionInfo, ApiError> {
    client::request(
        reqwest::Method::PUT,
        &format!("/api/v1/system/retention/orgs/{org_id}"),
        Some(serde_json::to_value(RetentionUpdate { days }).unwrap_or_default()),
    )
    .await
}

/// `PUT /api/v1/system/retention/default` — platform admin (self-hosted).
pub async fn set_default_retention(days: Option<u32>) -> Result<(), ApiError> {
    client::request_opt::<serde_json::Value>(
        reqwest::Method::PUT,
        "/api/v1/system/retention/default",
        Some(serde_json::to_value(RetentionUpdate { days }).unwrap_or_default()),
    )
    .await
    .map(|_| ())
}

/// `GET /api/v1/system/o2/streams` — O2 metrics streams of the current org.
pub async fn o2_streams() -> Result<O2StreamList, ApiError> {
    client::request(reqwest::Method::GET, "/api/v1/system/o2/streams", None).await
}

/// `DELETE /api/v1/system/o2/streams/{name}` — owner/admin.
pub async fn delete_stream(name: &str) -> Result<O2DeleteResult, ApiError> {
    client::request(
        reqwest::Method::DELETE,
        &format!("/api/v1/system/o2/streams/{name}"),
        None,
    )
    .await
}

/// `POST /api/v1/system/o2/streams/{name}/delete-range` — RFC 3339 bounds.
pub async fn delete_range(
    name: &str,
    start: String,
    end: String,
) -> Result<O2DeleteResult, ApiError> {
    client::request(
        reqwest::Method::POST,
        &format!("/api/v1/system/o2/streams/{name}/delete-range"),
        Some(serde_json::to_value(O2DeleteRangeRequest { start, end }).unwrap_or_default()),
    )
    .await
}

/// `POST /api/v1/system/o2/purge` — owner/admin, typed org name.
pub async fn purge(confirm: String) -> Result<O2DeleteResult, ApiError> {
    client::request(
        reqwest::Method::POST,
        "/api/v1/system/o2/purge",
        Some(serde_json::to_value(O2PurgeRequest { confirm }).unwrap_or_default()),
    )
    .await
}

/// `GET /api/v1/system/status` — platform admin.
pub async fn status() -> Result<SystemStatus, ApiError> {
    client::request(reqwest::Method::GET, "/api/v1/system/status", None).await
}

/// `GET /api/v1/system/orgs` — platform admin: every org with retention,
/// O2 usage and quota.
pub async fn orgs_overview() -> Result<Vec<OrgSystemRow>, ApiError> {
    client::request(reqwest::Method::GET, "/api/v1/system/orgs", None).await
}

/// `POST /api/v1/system/secrets/rekey` — platform admin: re-encrypts every
/// vault secret with the write key (secrets.md S8).
pub async fn rekey_secrets() -> Result<pnex_core::SecretsRekeyReport, ApiError> {
    client::request(reqwest::Method::POST, "/api/v1/system/secrets/rekey", None).await
}

/// `GET /api/v1/system/tiers` — platform admin.
pub async fn tiers() -> Result<Vec<TierOption>, ApiError> {
    client::request(reqwest::Method::GET, "/api/v1/system/tiers", None).await
}

/// `PUT /api/v1/system/orgs/{id}/tier` — platform admin; `None` removes it.
pub async fn set_org_tier(org_id: i64, tier_id: Option<i64>) -> Result<OrgTierUpdate, ApiError> {
    client::request(
        reqwest::Method::PUT,
        &format!("/api/v1/system/orgs/{org_id}/tier"),
        Some(serde_json::to_value(OrgTierUpdate { tier_id }).unwrap_or_default()),
    )
    .await
}
