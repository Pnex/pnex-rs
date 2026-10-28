//! OpenObserve integration (D1/D2): telemetry storage, 1 O2 org per PNEX
//! org, ingestion credentials provisioned automatically and kept
//! in the database (`openobserve_orgs`) — the ingestion path draws the
//! correlated org/token pair from it.
//!
//! API contract checked against openobserve v0.92.1 (dev compose):
//! - `GET /api/organizations` (Basic root) → `data[] {identifier, name}`;
//! - `POST /api/organizations {name}` → **does not deduplicate by name**
//!   (re-creating = 2nd org) → always search by name before creating;
//!   name: alphanumeric + underscore only;
//! - `POST /api/{identifier}/users {email,password,role:"admin"}` (the only
//!   native role besides root) → 400 « User already exists » if present;
//! - `PUT /api/{identifier}/users/{email} {new_password,change_password}`
//!   (root) → takes a user back without its old password;
//! - `GET /api/{identifier}/passcode` (Basic email:password of the user) →
//!   `data.passcode` (o2oi_…) — Bearer passcode NOT supported for ingestion;
//! - ingestion `POST /api/{identifier}/{stream}/_json` (Basic
//!   email:password **or** email:passcode, body = JSON array).

pub mod client;
pub mod promwrite;
pub mod provisioning;
pub mod sink;

pub use client::Client;
pub use provisioning::{ensure_org_credentials, provisioned_credentials, OrgCredentials};
pub use sink::spawn_batcher;

use loco_rs::config::Config;
use serde::Deserialize;

#[derive(Clone, Debug, Deserialize)]
pub struct OpenobserveSettings {
    pub base_url: String,
    /// Admin service account (org/user provisioning — never used
    /// to ingest).
    pub root_email: String,
    pub root_password: String,
}

impl OpenobserveSettings {
    /// Optional `settings.openobserve` — absent = feature
    /// disabled (noop sink, health « not-configured »; the test case).
    pub fn from_config(config: &Config) -> Option<Self> {
        config
            .settings
            .as_ref()
            .and_then(|s| s.get("openobserve"))
            .and_then(|v| serde_json::from_value(v.clone()).ok())
    }
}

/// Valid Prometheus metric name (official charset) — the function lives
/// in pnex-core (`naming`): single source with the flow runtime.
pub(crate) use pnex_core::valid_metric_name;
