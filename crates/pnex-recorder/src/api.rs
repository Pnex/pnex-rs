//! PNeX server calls: password sign-in, org list, speech-to-text models and
//! the model test endpoint. The session (refresh token) is kept on disk so the
//! tray utility starts signed in.

use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use pnex_core::media_ingest::{AsrModel, AsrProfile, AsrTask, AsrTestResult};
use pnex_core::vision::ModelCheckStatus;
use pnex_core::{OrgMembership, Paginated, TokenResponse, UserInfo};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

/// Rauthy only accepts a refresh in the access token's last minute.
const REFRESH_WINDOW: u64 = 60;

#[derive(Debug, Clone, PartialEq)]
pub enum ApiError {
    Unreachable,
    NotPnex,
    /// Server contract number.
    Incompatible(u32),
    BadCredentials,
    /// HTTPS server signed by its own root CA (local edge, D70): the user
    /// compares the fingerprint before the CA is pinned.
    UntrustedCa {
        pem: String,
        fingerprint: String,
    },
    SessionExpired,
    /// HTTP status and `ErrorDetail.error` code when the server sent one.
    Server(u16, Option<String>),
}

#[derive(Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Session {
    pub url: String,
    pub email: String,
    pub org: Option<i64>,
    /// Capture switched on from the window or the tray.
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    refresh_token: String,
    #[serde(default)]
    access_token: String,
    #[serde(default)]
    expires_at: u64,
    /// Root CA pinned after the user confirmed its fingerprint.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    ca_pem: Option<String>,
}

fn path() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("APPDATA").map(PathBuf::from))
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))?;
    Some(base.join("pnex").join("recorder.json"))
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

/// Client trusting the webpki roots plus the pinned CA, if any.
fn client(ca: Option<&str>) -> reqwest::Client {
    let mut builder = reqwest::Client::builder()
        .user_agent(concat!("pnex-recorder/", env!("CARGO_PKG_VERSION")))
        .timeout(std::time::Duration::from_secs(60));
    // An unparsable PEM is skipped: the handshake then fails, the safe outcome.
    if let Some(cert) = ca.and_then(|pem| reqwest::Certificate::from_pem(pem.as_bytes()).ok()) {
        builder = builder.add_root_certificate(cert);
    }
    builder.build().expect("reqwest client")
}

/// The server's root CA, read before any trust exists: accepts any
/// certificate, so it only fetches this public file and never carries tokens.
async fn discover_ca(url: &str) -> Option<String> {
    let pem = reqwest::Client::builder()
        .danger_accept_invalid_certs(true)
        .timeout(std::time::Duration::from_secs(10))
        .build()
        .ok()?
        .get(format!("{url}{}", pnex_api_contract::META_CA_PATH))
        .send()
        .await
        .ok()?
        .error_for_status()
        .ok()?
        .text()
        .await
        .ok()?;
    pem.contains("-----BEGIN CERTIFICATE-----").then_some(pem)
}

/// SHA-256 fingerprint of the first certificate of `pem`, uppercase hex pairs
/// joined by `:` (same format as the PNeX app and `openssl x509 -fingerprint`).
pub fn fingerprint(pem: &str) -> Option<String> {
    use base64::Engine;
    use sha2::Digest;
    let body: String = pem
        .lines()
        .skip_while(|line| !line.starts_with("-----BEGIN CERTIFICATE-----"))
        .skip(1)
        .take_while(|line| !line.starts_with("-----END CERTIFICATE-----"))
        .map(str::trim)
        .collect();
    let der = base64::engine::general_purpose::STANDARD
        .decode(body)
        .ok()?;
    if der.is_empty() {
        return None;
    }
    let hex: Vec<String> = sha2::Sha256::digest(&der)
        .iter()
        .map(|b| format!("{b:02X}"))
        .collect();
    Some(hex.join(":"))
}

async fn error_of(res: reqwest::Response) -> ApiError {
    let status = res.status().as_u16();
    let body: Value = res.json().await.unwrap_or(Value::Null);
    ApiError::Server(status, body["error"].as_str().map(str::to_owned))
}

impl Session {
    pub fn load() -> Option<Self> {
        serde_json::from_slice(&std::fs::read(path()?).ok()?).ok()
    }

    pub fn save(&self) {
        let Some(path) = path() else { return };
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let mut opts = std::fs::OpenOptions::new();
        opts.write(true).create(true).truncate(true);
        // The file holds the refresh token: owner only.
        #[cfg(unix)]
        std::os::unix::fs::OpenOptionsExt::mode(&mut opts, 0o600);
        if let Ok(mut file) = opts.open(&path) {
            #[cfg(unix)]
            let _ = file.set_permissions(std::os::unix::fs::PermissionsExt::from_mode(0o600));
            let _ = std::io::Write::write_all(
                &mut file,
                &serde_json::to_vec_pretty(self).unwrap_or_default(),
            );
        }
    }

    /// The CA pinned for `url`, kept across sign-outs of the same server.
    pub fn pinned_ca(&self, url: &str) -> Option<String> {
        (self.url == url.trim().trim_end_matches('/'))
            .then(|| self.ca_pem.clone())
            .flatten()
    }

    pub fn signed_in(&self) -> bool {
        !self.refresh_token.is_empty()
    }

    /// Forgets the tokens, keeps server and email to prefill the sign-in.
    pub fn sign_out(&mut self) {
        // The pinned CA stays: it belongs to the server, not to the account.
        self.refresh_token.clear();
        self.access_token.clear();
        self.expires_at = 0;
        self.save();
    }

    fn set_tokens(&mut self, tokens: TokenResponse) {
        self.access_token = tokens.access_token;
        self.refresh_token = tokens.refresh_token;
        self.expires_at = now() + tokens.expires_in.max(0) as u64;
    }

    fn http(&self) -> reqwest::Client {
        client(self.ca_pem.as_deref())
    }

    /// Checks the server is a compatible PNeX, then signs in with the password
    /// grant. `ca` is a root CA the user just confirmed (or the pinned one).
    pub async fn sign_in(
        url: &str,
        email: &str,
        password: &str,
        ca: Option<String>,
    ) -> Result<Self, ApiError> {
        let url = url.trim().trim_end_matches('/').to_string();
        let probe = client(ca.as_deref())
            .get(format!("{url}{}", pnex_api_contract::META_VERSION_PATH))
            .send()
            .await;
        let probe = match probe {
            Ok(res) => res,
            // TLS refused by an unknown authority: offer the server's own CA.
            Err(_) if ca.is_none() && url.starts_with("https://") => {
                let pem = discover_ca(&url).await.ok_or(ApiError::Unreachable)?;
                let fingerprint = fingerprint(&pem).ok_or(ApiError::Unreachable)?;
                return Err(ApiError::UntrustedCa { pem, fingerprint });
            }
            Err(_) => return Err(ApiError::Unreachable),
        };
        let info: pnex_api_contract::ServerInfo =
            probe.json().await.map_err(|_| ApiError::NotPnex)?;
        if info.service != pnex_api_contract::SERVICE {
            return Err(ApiError::NotPnex);
        }
        if !pnex_api_contract::compatible(info.contract, pnex_api_contract::CONTRACT) {
            return Err(ApiError::Incompatible(info.contract));
        }
        let body =
            json!({"grant_type": "password", "username": email.trim(), "password": password});
        let tokens = token(&client(ca.as_deref()), &url, &body)
            .await
            .map_err(|e| match e {
                ApiError::Server(400 | 401, _) => ApiError::BadCredentials,
                e => e,
            })?;
        let mut session = Self {
            url,
            email: email.trim().to_string(),
            ca_pem: ca,
            ..Self::default()
        };
        session.set_tokens(tokens);
        session.save();
        Ok(session)
    }

    /// A valid access token, refreshed (and saved) when about to expire.
    async fn bearer(&mut self) -> Result<String, ApiError> {
        if self.expires_at.saturating_sub(now()) < REFRESH_WINDOW {
            let body = json!({"grant_type": "refresh_token", "refresh_token": self.refresh_token});
            match token(&self.http(), &self.url, &body).await {
                Ok(tokens) => {
                    self.set_tokens(tokens);
                    self.save();
                }
                // Refresh not open yet (clock skew): the access token still works.
                Err(_) if self.expires_at > now() => {}
                Err(ApiError::Unreachable) => return Err(ApiError::Unreachable),
                Err(_) => return Err(ApiError::SessionExpired),
            }
        }
        Ok(self.access_token.clone())
    }

    pub async fn orgs(&mut self) -> Result<Vec<OrgMembership>, ApiError> {
        let bearer = self.bearer().await?;
        let res = self
            .http()
            .get(format!("{}/api/v1/user-info", self.url))
            .bearer_auth(bearer)
            .send()
            .await
            .map_err(|_| ApiError::Unreachable)?;
        if !res.status().is_success() {
            return Err(error_of(res).await);
        }
        let info: UserInfo = res.json().await.map_err(|_| ApiError::NotPnex)?;
        Ok(info.orgs)
    }

    /// Speech-to-text models saved in the org (the other audio tasks cannot
    /// transcribe).
    pub async fn models(&mut self, org: i64) -> Result<Vec<AsrModel>, ApiError> {
        let bearer = self.bearer().await?;
        let res = self
            .http()
            .get(format!("{}/api/v1/asr/models?limit=100", self.url))
            .bearer_auth(bearer)
            .header("X-Org-Id", org.to_string())
            .send()
            .await
            .map_err(|_| ApiError::Unreachable)?;
        if !res.status().is_success() {
            return Err(error_of(res).await);
        }
        let page: Paginated<AsrModel> = res.json().await.map_err(|_| ApiError::NotPnex)?;
        Ok(page
            .results
            .into_iter()
            .filter(|m| m.task == AsrTask::Asr)
            .collect())
    }

    /// The org's default transcription profile, if one is set.
    pub async fn default_profile(&mut self, org: i64) -> Result<Option<AsrProfile>, ApiError> {
        let bearer = self.bearer().await?;
        let res = self
            .http()
            .get(format!("{}/api/v1/asr/profiles?limit=100", self.url))
            .bearer_auth(bearer)
            .header("X-Org-Id", org.to_string())
            .send()
            .await
            .map_err(|_| ApiError::Unreachable)?;
        if !res.status().is_success() {
            return Err(error_of(res).await);
        }
        let page: Paginated<AsrProfile> = res.json().await.map_err(|_| ApiError::NotPnex)?;
        Ok(page.results.into_iter().find(|p| p.is_default))
    }

    /// Transcribes a WAV clip with `model`; nothing is stored server side.
    pub async fn test(
        &mut self,
        org: i64,
        model: &str,
        language: &str,
        wav: Vec<u8>,
    ) -> Result<AsrTestResult, ApiError> {
        let bearer = self.bearer().await?;
        let res = self
            .http()
            .post(format!(
                "{}/api/v1/asr/models/{model}/test?name=clip.wav&language={language}",
                self.url
            ))
            .bearer_auth(bearer)
            .header("X-Org-Id", org.to_string())
            .header("Content-Type", "audio/wav")
            .body(wav)
            .send()
            .await
            .map_err(|_| ApiError::Unreachable)?;
        if !res.status().is_success() {
            return Err(error_of(res).await);
        }
        res.json().await.map_err(|_| ApiError::NotPnex)
    }
}

/// Only models that passed their check can be tested (409 otherwise).
pub fn usable(model: &AsrModel) -> bool {
    model.check.status == ModelCheckStatus::Valid
}

async fn token(http: &reqwest::Client, url: &str, body: &Value) -> Result<TokenResponse, ApiError> {
    let res = http
        .post(format!("{url}/api/v1/oauth2/token"))
        .json(body)
        .send()
        .await
        .map_err(|_| ApiError::Unreachable)?;
    if !res.status().is_success() {
        return Err(error_of(res).await);
    }
    res.json().await.map_err(|_| ApiError::NotPnex)
}

#[cfg(test)]
mod tests {
    #[test]
    fn fingerprint_hashes_the_der_body() {
        // DER = b"hello" -> SHA-256 2CF24DBA...
        let pem = "junk\n-----BEGIN CERTIFICATE-----\naGVs\nbG8=\n-----END CERTIFICATE-----\n";
        let fp = super::fingerprint(pem).expect("fingerprint");
        assert!(fp.starts_with("2C:F2:4D:BA:5F:B0:A3:0E"), "{fp}");
        assert_eq!(fp.split(':').count(), 32);
        assert_eq!(super::fingerprint("not a certificate"), None);
    }
}
