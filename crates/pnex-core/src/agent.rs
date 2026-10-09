//! Edge agent contract (D95, docs/architecture/edge-agent.md) — DTOs shared
//! by the backend, the frontend (wasm) and the `pnex-agent` binary.
//!
//! Dates are RFC 3339 strings (no chrono in the core, wasm32).

use serde::{Deserialize, Serialize};

/// Local API default bind of the agent (loopback only, no password).
pub const AGENT_DEFAULT_LISTEN: &str = "127.0.0.1:7070";

/// Lifetime of an enrollment code, in seconds.
pub const AGENT_ENROLL_TTL_SECS: i64 = 15 * 60;

/// Downloadable agent builds: `(target id, published file name)`. The id is
/// what the install scripts compute from `uname`/`$env:PROCESSOR_ARCHITECTURE`.
pub const AGENT_TARGETS: &[(&str, &str)] = &[
    ("x86_64-linux", "pnex-agent-x86_64-linux"),
    ("aarch64-linux", "pnex-agent-aarch64-linux"),
    ("armv7-linux", "pnex-agent-armv7-linux"),
    ("x86_64-windows", "pnex-agent-x86_64-windows.exe"),
];

/// File name of a target id (`None` = unknown target).
pub fn agent_file_of(target: &str) -> Option<&'static str> {
    AGENT_TARGETS
        .iter()
        .find(|(t, _)| *t == target)
        .map(|(_, f)| *f)
}

/// Canonical form of a typed enrollment code: uppercase, separators and
/// blanks removed (`abcd-efgh ijkl` → `ABCDEFGHIJKL`).
pub fn normalize_enroll_code(raw: &str) -> String {
    raw.chars()
        .filter(char::is_ascii_alphanumeric)
        .map(|c| c.to_ascii_uppercase())
        .collect()
}

/// One key discovered from an agent (`GET /api/v1/devices/{id}/agent-keys`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AgentKey {
    pub id: i64,
    /// Normalized series name.
    pub key: String,
    #[serde(default)]
    pub unit: Option<String>,
    /// `number` | `bool` | `text` | `json`.
    pub kind: String,
    /// OpenObserve history on/off (Valkey live cache is always on).
    pub record_o2: bool,
    pub first_seen_at: String,
    pub last_seen_at: String,
}

/// `PATCH /api/v1/devices/{id}/agent-keys/{key_id}`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AgentKeyPatch {
    pub record_o2: bool,
}

/// `GET|PATCH /api/v1/devices/{id}/agent` — agent overview + settings.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct AgentInfo {
    /// Distinct keys quota.
    pub max_keys: i32,
    /// Agent version from its last announce.
    #[serde(default)]
    pub agent_version: Option<String>,
    /// Machine facts reported at the last successful enrollment.
    #[serde(default)]
    pub hostname: Option<String>,
    #[serde(default)]
    pub os: Option<String>,
    #[serde(default)]
    pub arch: Option<String>,
    #[serde(default)]
    pub enrolled_at: Option<String>,
}

/// `PATCH /api/v1/devices/{id}/agent`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AgentSettingsPatch {
    pub max_keys: i32,
}

/// `POST /api/v1/devices/{id}/agent-enrollment` → a fresh single-use code
/// (previous pending codes of the device are revoked).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AgentEnrollmentCreated {
    /// Display form, `XXXX-XXXX-XXXX`.
    pub code: String,
    pub expires_at: String,
    /// Hex SHA-256 of the exact `/api/v1/meta/ca` body (`None` = no local CA:
    /// the server certificate is publicly trusted).
    #[serde(default)]
    pub ca_sha256: Option<String>,
}

/// `POST /api/v1/agent/enroll` (public) — body sent by `pnex-agent install`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AgentEnrollRequest {
    pub code: String,
    #[serde(default)]
    pub hostname: Option<String>,
    #[serde(default)]
    pub os: Option<String>,
    #[serde(default)]
    pub arch: Option<String>,
    #[serde(default)]
    pub agent_version: Option<String>,
}

/// Credentials returned once to the enrolling agent. Enrolling rotates the
/// device token and key: any previous installation is disconnected (4005).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AgentEnrollResponse {
    pub device_id: String,
    pub token: String,
    /// Base64 32-byte pre-shared key of the Noise link (D156).
    pub encryption_key: String,
    /// WebSocket path on the same origin (`/ws/device`).
    pub ws_path: String,
    /// Local root CA PEM to pin (`None` = publicly trusted certificate).
    #[serde(default)]
    pub ca_pem: Option<String>,
    /// TLS client certificate + PKCS#8 key (PEM) issued by the org CA
    /// (D153). Every enrollment revokes the previous ones.
    pub client_cert_pem: String,
    pub client_key_pem: String,
    /// Device endpoint (`host[:port]`, D158) the agent connects to over
    /// wss.
    pub device_host: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn enroll_code_normalization() {
        assert_eq!(normalize_enroll_code(" abcd-efgh ijkl\n"), "ABCDEFGHIJKL");
    }

    #[test]
    fn target_files() {
        assert_eq!(
            agent_file_of("x86_64-windows"),
            Some("pnex-agent-x86_64-windows.exe")
        );
        assert_eq!(agent_file_of("../etc/passwd"), None);
    }
}
