//! Edge agent endpoints (D95): overview + quota, discovered keys (per-key
//! OpenObserve recording toggle), single-use enrollment codes.

use pnex_core::agent::{AgentEnrollmentCreated, AgentInfo, AgentKey};

use crate::api::client;
use crate::api::error::ApiError;

/// `GET /api/v1/devices/{id}/agent`.
pub async fn info(id: i64) -> Result<AgentInfo, ApiError> {
    client::request(
        reqwest::Method::GET,
        &format!("/api/v1/devices/{id}/agent"),
        None,
    )
    .await
}

/// `PATCH /api/v1/devices/{id}/agent` — distinct keys quota.
pub async fn set_max_keys(id: i64, max_keys: i32) -> Result<AgentInfo, ApiError> {
    client::request(
        reqwest::Method::PATCH,
        &format!("/api/v1/devices/{id}/agent"),
        Some(serde_json::json!({ "max_keys": max_keys })),
    )
    .await
}

/// `GET /api/v1/devices/{id}/agent-keys`.
pub async fn keys(id: i64) -> Result<Vec<AgentKey>, ApiError> {
    client::request(
        reqwest::Method::GET,
        &format!("/api/v1/devices/{id}/agent-keys"),
        None,
    )
    .await
}

/// `PATCH /api/v1/devices/{id}/agent-keys/{key_id}` — OpenObserve history.
pub async fn set_record(id: i64, key_id: i64, record_o2: bool) -> Result<AgentKey, ApiError> {
    client::request(
        reqwest::Method::PATCH,
        &format!("/api/v1/devices/{id}/agent-keys/{key_id}"),
        Some(serde_json::json!({ "record_o2": record_o2 })),
    )
    .await
}

/// `DELETE /api/v1/devices/{id}/agent-keys/{key_id}` — forget a stray key.
pub async fn forget_key(id: i64, key_id: i64) -> Result<(), ApiError> {
    client::request_opt::<serde_json::Value>(
        reqwest::Method::DELETE,
        &format!("/api/v1/devices/{id}/agent-keys/{key_id}"),
        None,
    )
    .await
    .map(|_| ())
}

/// `POST /api/v1/devices/{id}/agent-enrollment` — fresh single-use code.
pub async fn new_enrollment(id: i64) -> Result<AgentEnrollmentCreated, ApiError> {
    client::request(
        reqwest::Method::POST,
        &format!("/api/v1/devices/{id}/agent-enrollment"),
        None,
    )
    .await
}

/// Install one-liners shown by the UI (pure, unit-tested). `server` has no
/// trailing slash; `ca_sha256` = pin the local CA (TOFU by fingerprint).
pub struct InstallCommands {
    pub linux: String,
    pub windows: String,
    pub manual: String,
}

pub fn install_commands(server: &str, code: &str, ca_sha256: Option<&str>) -> InstallCommands {
    let s = server.trim().trim_end_matches('/');
    match ca_sha256 {
        Some(h) => InstallCommands {
            linux: format!(
                "curl -fsSk {s}/api/v1/meta/ca -o /tmp/pnex-ca.pem && echo \"{h}  /tmp/pnex-ca.pem\" | sha256sum -c - && curl -fsSL --cacert /tmp/pnex-ca.pem {s}/api/v1/agent/install.sh | sudo sh -s -- --server {s} --enroll {code} --ca-sha256 {h}"
            ),
            windows: format!(
                "$u='{s}';$h='{h}';[Net.ServicePointManager]::ServerCertificateValidationCallback={{$true}};$c=(New-Object Net.WebClient).DownloadData(\"$u/api/v1/meta/ca\");if(([BitConverter]::ToString([Security.Cryptography.SHA256]::Create().ComputeHash($c)) -replace '-','').ToLower() -ne $h){{throw 'CA fingerprint mismatch'}};$global:PnexCa=New-Object Security.Cryptography.X509Certificates.X509Certificate2(,$c);[Net.ServicePointManager]::ServerCertificateValidationCallback={{param($a,$b,$ch,$e) $x=New-Object Security.Cryptography.X509Certificates.X509Chain;$x.ChainPolicy.ExtraStore.Add($global:PnexCa)|Out-Null;$x.ChainPolicy.RevocationMode='NoCheck';$x.ChainPolicy.VerificationFlags='AllowUnknownCertificateAuthority';$null=$x.Build($b);$x.ChainElements[$x.ChainElements.Count-1].Certificate.Thumbprint -eq $global:PnexCa.Thumbprint}};$p=(New-Object Net.WebClient).DownloadString(\"$u/api/v1/agent/install.ps1\");& ([scriptblock]::Create($p)) -Server $u -Enroll '{code}' -CaSha256 $h"
            ),
            manual: format!("pnex-agent install --server {s} --enroll {code} --ca-sha256 {h}"),
        },
        None => InstallCommands {
            linux: format!(
                "curl -fsSL {s}/api/v1/agent/install.sh | sudo sh -s -- --server {s} --enroll {code}"
            ),
            windows: format!(
                "$u='{s}';$p=(New-Object Net.WebClient).DownloadString(\"$u/api/v1/agent/install.ps1\");& ([scriptblock]::Create($p)) -Server $u -Enroll '{code}'"
            ),
            manual: format!("pnex-agent install --server {s} --enroll {code}"),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pinned_commands_carry_fingerprint_and_code() {
        let c = install_commands("https://pnex.local/", "ABCD-EFGH-JKMN", Some("ab12"));
        assert!(c.linux.contains("sha256sum -c -"));
        assert!(c
            .linux
            .contains("--cacert /tmp/pnex-ca.pem https://pnex.local/api/v1/agent/install.sh"));
        assert!(c
            .linux
            .ends_with("--enroll ABCD-EFGH-JKMN --ca-sha256 ab12"));
        assert!(c.windows.contains("-Enroll 'ABCD-EFGH-JKMN' -CaSha256 $h"));
        assert!(c.windows.contains("$h='ab12'"));
        assert!(!c.windows.contains("{{"));
    }

    #[test]
    fn public_certificate_commands_have_no_pinning() {
        let c = install_commands("https://pnex.example", "X", None);
        assert!(!c.linux.contains("meta/ca"));
        assert!(c.manual.ends_with("--enroll X"));
    }
}
