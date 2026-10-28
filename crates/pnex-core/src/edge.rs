//! Edge references — WiFi credentials + PNeX server hosts (org-scoped).
//!
//! Feeds the Config step of the device wizard: no more re-typing WiFi/host
//! on every addition, the entry lives in the database per org. The `CreateBuild` payload
//! is **unchanged** (the wizard resolves entry → values) — a purely
//! additive change, CONTRACT unchanged (2).
//!
//! WiFi password: a vault secret (secrets.md lot S6) — the API only carries
//! its reference and name, never the value; builds reference the
//! credential and the build worker decrypts at run time. wasm-safe (serde
//! only, dates as RFC 3339 String).

use serde::{Deserialize, Serialize};

/// Saved WiFi credential — `GET /api/v1/edge/wifi-credentials`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[cfg_attr(
    feature = "openapi",
    schema(
        description = "Saved WiFi credential of the org (`GET /api/v1/edge/wifi-credentials`). The password is a vault reference, never a value."
    )
)]
pub struct WifiCredential {
    pub id: i64,
    pub org_id: i64,
    /// Case-sensitive (byte-sensitive 802.11), so is the upsert.
    pub ssid: String,
    /// Vault secret of the password (`None` = not set).
    #[serde(default)]
    pub password: Option<crate::SecretFieldView>,
    /// RFC 3339.
    #[cfg_attr(feature = "openapi", schema(format = DateTime))]
    pub created_at: String,
    /// RFC 3339.
    #[cfg_attr(feature = "openapi", schema(format = DateTime))]
    pub updated_at: String,
}

/// Body of `POST /api/v1/edge/wifi-credentials` (upsert on (org, ssid)).
///
/// `password`: pick a vault secret or type a value (owner/admin, stored in
/// the dedicated secret `wifi/<ssid>/password`). Absent on an update =
/// unchanged. Unknown fields are refused.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[cfg_attr(
    feature = "openapi",
    schema(
        description = "Body of the WiFi credential upsert/update. `password`: pick a vault secret or type a value (owner/admin); absent on update = unchanged. Unknown fields are refused."
    )
)]
#[serde(deny_unknown_fields)]
pub struct WifiCredentialInput {
    pub ssid: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub password: Option<crate::SecretFieldInput>,
}

/// Saved PNeX server host — `GET /api/v1/edge/hosts`.
/// `GET /api/v1/edge/hosts/locked` — server host imposed by the deployment
/// (`PNEX_PROD_HOST`). `host: None` = free choice from the org's referential.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[cfg_attr(
    feature = "openapi",
    schema(
        description = "Server host imposed by the deployment (`PNEX_PROD_HOST`); `host: null` = free choice from the org referential."
    )
)]
pub struct LockedHost {
    pub host: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[cfg_attr(
    feature = "openapi",
    schema(description = "Saved PNeX server host of the org (`GET /api/v1/edge/hosts`).")
)]
pub struct PnexHost {
    pub id: i64,
    pub org_id: i64,
    /// Bare host without scheme (`192.168.1.16`, `dev1.pnex.io`): devices
    /// always connect over wss (D154).
    pub host: String,
    /// RFC 3339.
    #[cfg_attr(feature = "openapi", schema(format = DateTime))]
    pub created_at: String,
    /// RFC 3339.
    #[cfg_attr(feature = "openapi", schema(format = DateTime))]
    pub updated_at: String,
}

/// Body of `POST /api/v1/edge/hosts` (upsert on (org, host)).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(deny_unknown_fields)]
pub struct PnexHostInput {
    pub host: String,
}

/// A PNeX server detected by the server-side LAN scan —
/// `GET /api/v1/edge/lan-scan`. The `pnex-server` identity is checked by
/// the meta probe (`service` field of the contract), not just an open port.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[cfg_attr(
    feature = "openapi",
    schema(
        description = "A PNeX server found by the server-side LAN scan (identity checked through the meta probe)."
    )
)]
pub struct LanScanHit {
    /// Bare host without scheme, ready for [`PnexHost::host`].
    pub host: String,
    /// Version of the detected server (informative).
    pub version: String,
    /// API contract of the detected server (informative).
    pub contract: u32,
}

/// LAN scan response — the "a.b.c." prefixes actually probed
/// (auto-detection of the server's private interfaces, or prefix passed in
/// the query) and the servers found (sorted by host).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[cfg_attr(
    feature = "openapi",
    schema(
        description = "LAN scan answer: the probed `a.b.c.` prefixes and the servers found, sorted by host."
    )
)]
pub struct LanScanResult {
    pub prefixes: Vec<String>,
    pub hits: Vec<LanScanHit>,
}
