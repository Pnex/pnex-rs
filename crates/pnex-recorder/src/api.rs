//! PNeX server calls: browser sign-in, org list, speech-to-text models and
//! the model test endpoint. The session (refresh token) is kept on disk so the
//! tray utility starts signed in.

use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use pnex_core::media_analysis::{AnalysisStep, MediaAnalysis};
use pnex_core::media_ingest::{AsrModel, AsrProfile, AsrTask, AsrTestResult};
use pnex_core::ontology::api::{ObjectView, QueryResult};
use pnex_core::vision::ModelCheckStatus;
use pnex_core::voice_id::{VoiceProfileInput, VoiceProfileView, VoiceTestResult, PERSON_TYPE};
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
    /// The browser sign-in was not completed within ten minutes.
    LoginTimedOut,
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
    /// "light" or "dark" once the user picked one; the system's otherwise.
    /// Kept across sign-outs, like the server.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub theme: Option<String>,
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

    /// Checks the server is a compatible PNeX, then signs in through the
    /// system browser (PKCE code flow, relayed by the server's native bridge
    /// like the desktop app). `ca` is a root CA the user just confirmed (or
    /// the pinned one).
    pub async fn sign_in(url: &str, ca: Option<String>) -> Result<Self, ApiError> {
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
        let http = client(ca.as_deref());
        let tokens = browser_login(&http, &url).await?;
        let mut session = Self {
            url,
            ca_pem: ca,
            ..Self::default()
        };
        session.set_tokens(tokens);
        // The email names the voice profile and its consent proof.
        session.email = session.user_info().await?.email.unwrap_or_default();
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
        Ok(self.user_info().await?.orgs)
    }

    async fn user_info(&mut self) -> Result<UserInfo, ApiError> {
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
        res.json().await.map_err(|_| ApiError::NotPnex)
    }

    /// Speech-to-text models saved in the org (the other audio tasks cannot
    /// transcribe).
    pub async fn models(&mut self, org: i64) -> Result<Vec<AsrModel>, ApiError> {
        self.models_of(org, AsrTask::Asr).await
    }

    /// Audio models of the org for one task.
    async fn models_of(&mut self, org: i64, task: AsrTask) -> Result<Vec<AsrModel>, ApiError> {
        let page: Paginated<AsrModel> = self
            .org_json(org, |h, u| {
                h.get(format!("{u}/api/v1/asr/models?limit=100"))
            })
            .await?;
        Ok(page
            .results
            .into_iter()
            .filter(|m| m.task == task)
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

impl Session {
    /// Stores one Ogg/Opus part of a recording in the org's media library;
    /// returns its asset id.
    pub async fn upload_recording(
        &mut self,
        org: i64,
        bytes: Vec<u8>,
        filename: &str,
        name: &str,
    ) -> Result<String, ApiError> {
        let bearer = self.bearer().await?;
        let res = self
            .http()
            .post(format!("{}/api/v1/media", self.url))
            .query(&[("name", name), ("filename", filename), ("kind", "audio")])
            .bearer_auth(bearer)
            .header("X-Org-Id", org.to_string())
            .header("Content-Type", "audio/ogg")
            .timeout(std::time::Duration::from_secs(600))
            .body(bytes)
            .send()
            .await
            .map_err(|_| ApiError::Unreachable)?;
        if !res.status().is_success() {
            return Err(error_of(res).await);
        }
        let asset: Value = res.json().await.map_err(|_| ApiError::NotPnex)?;
        asset["id"]
            .as_str()
            .map(str::to_owned)
            .ok_or(ApiError::NotPnex)
    }

    /// Labels a stored recording (`source: recorder`, `app: <call app>`): the
    /// recorder's "Recordings" link filters the media library on them.
    pub async fn label_recording(
        &mut self,
        org: i64,
        asset: &str,
        app: &str,
    ) -> Result<(), ApiError> {
        let bearer = self.bearer().await?;
        let res = self
            .http()
            .put(format!(
                "{}/api/v1/resources/media_asset/{asset}/labels",
                self.url
            ))
            .bearer_auth(bearer)
            .header("X-Org-Id", org.to_string())
            .json(&json!({"labels": {"source": "recorder", "app": app.to_lowercase()}}))
            .send()
            .await
            .map_err(|_| ApiError::Unreachable)?;
        if !res.status().is_success() {
            return Err(error_of(res).await);
        }
        Ok(())
    }

    /// Files a sent call in the user's case ("Calls of <email>", created on
    /// first use and found again by its labels): one dated event, `from` →
    /// `to`, with the media of every part attached as evidence.
    pub async fn file_call(
        &mut self,
        org: i64,
        case_title: &str,
        title: &str,
        from: chrono::DateTime<chrono::Utc>,
        to: chrono::DateTime<chrono::Utc>,
        assets: &[String],
    ) -> Result<(), ApiError> {
        let case = self.own_case(org, case_title).await?;
        let mut attach = Vec::new();
        for asset in assets {
            let object: ObjectView = self
                .org_json(org, |h, u| {
                    h.get(format!("{u}/api/v1/ontology/resolve/media_asset/{asset}"))
                })
                .await?;
            attach.push(json!({"object_id": object.id}));
        }
        let stamp = |t: chrono::DateTime<chrono::Utc>| t.format("%Y-%m-%dT%H:%M:%SZ").to_string();
        let body = json!({
            "title": title,
            "origin": "recorder",
            "occurred": {"from": stamp(from), "to": stamp(to), "precision": "range"},
            "attach": attach,
        });
        self.org_send(org, |h, u| {
            h.post(format!("{u}/api/v1/cases/{case}/events"))
                .json(&body)
        })
        .await?;
        Ok(())
    }

    /// The user's own case in `org`: labelled `recorder-owner: <email>`.
    async fn own_case(&mut self, org: i64, title: &str) -> Result<String, ApiError> {
        let label = format!("recorder-owner:{}", self.email);
        let found: QueryResult = self
            .org_json(org, |h, u| {
                h.post(format!("{u}/api/v1/ontology/query"))
                    .json(&json!({"type_key": "case", "label": label, "limit": 1}))
            })
            .await?;
        if let Some(row) = found.rows.into_iter().next() {
            return Ok(row.object.id);
        }
        let case: ObjectView = self
            .org_json(org, |h, u| {
                h.post(format!("{u}/api/v1/cases"))
                    .json(&json!({"title": title}))
            })
            .await?;
        let labels = json!({"labels": {"source": "recorder", "recorder-owner": self.email}});
        let native = case.native_id.clone();
        self.org_send(org, |h, u| {
            h.put(format!("{u}/api/v1/resources/case/{native}/labels"))
                .json(&labels)
        })
        .await?;
        Ok(case.id)
    }

    /// Starts the transcription of a stored recording (the org's default
    /// transcription profile runs it).
    pub async fn transcribe(
        &mut self,
        org: i64,
        asset: &str,
        locale: &str,
    ) -> Result<(), ApiError> {
        let bearer = self.bearer().await?;
        let res = self
            .http()
            .post(format!("{}/api/v1/media/{asset}/analyses", self.url))
            .bearer_auth(bearer)
            .header("X-Org-Id", org.to_string())
            .json(&json!({"steps": ["transcribe"], "locale": locale}))
            .send()
            .await
            .map_err(|_| ApiError::Unreachable)?;
        if !res.status().is_success() {
            return Err(error_of(res).await);
        }
        Ok(())
    }
}

impl Session {
    /// State of the transcription of a stored recording: `None` until the
    /// server lists the step.
    pub async fn transcription(
        &mut self,
        org: i64,
        asset: &str,
    ) -> Result<Option<MediaAnalysis>, ApiError> {
        #[derive(Deserialize)]
        struct State {
            analyses: Vec<MediaAnalysis>,
        }
        let bearer = self.bearer().await?;
        let res = self
            .http()
            .get(format!("{}/api/v1/media/{asset}/analyses", self.url))
            .bearer_auth(bearer)
            .header("X-Org-Id", org.to_string())
            .send()
            .await
            .map_err(|_| ApiError::Unreachable)?;
        if !res.status().is_success() {
            return Err(error_of(res).await);
        }
        let state: State = res.json().await.map_err(|_| ApiError::NotPnex)?;
        Ok(state
            .analyses
            .into_iter()
            .find(|a| a.step == AnalysisStep::Transcribe))
    }
}

/// Consent proof written on the voice profile the recorder creates: the
/// recorder finds the user's own profile again by it.
fn self_proof(email: &str) -> String {
    format!("PNeX recorder: self-enrolment of {email}")
}

/// What the user's own voice enrolment needs in the org.
#[derive(Clone, PartialEq)]
pub struct VoiceSetup {
    /// The user's profile, created by the recorder.
    pub profile: Option<VoiceProfileView>,
    /// Checked speaker embedding model of the org.
    pub model: Option<String>,
    /// May enrol (owner/admin: `voice-id:write`).
    pub can_enrol: bool,
}

impl Session {
    /// Sends `req` in `org`; the response of a success.
    async fn org_send(
        &mut self,
        org: i64,
        req: impl FnOnce(&reqwest::Client, &str) -> reqwest::RequestBuilder,
    ) -> Result<reqwest::Response, ApiError> {
        let bearer = self.bearer().await?;
        let res = req(&self.http(), &self.url)
            .bearer_auth(bearer)
            .header("X-Org-Id", org.to_string())
            .send()
            .await
            .map_err(|_| ApiError::Unreachable)?;
        if !res.status().is_success() {
            return Err(error_of(res).await);
        }
        Ok(res)
    }

    async fn org_json<T: serde::de::DeserializeOwned>(
        &mut self,
        org: i64,
        req: impl FnOnce(&reqwest::Client, &str) -> reqwest::RequestBuilder,
    ) -> Result<T, ApiError> {
        let res = self.org_send(org, req).await?;
        res.json().await.map_err(|_| ApiError::NotPnex)
    }

    pub async fn voice_setup(&mut self, org: i64) -> Result<VoiceSetup, ApiError> {
        let me: Value = self
            .org_json(org, |h, u| h.get(format!("{u}/api/v1/iam/me/permissions")))
            .await?;
        let can_enrol = me["permissions"]
            .as_array()
            .is_some_and(|p| p.iter().any(|p| p == "voice-id:write"));
        // shortcut: the first 100 profiles only; page when an org enrols more.
        let page: Paginated<VoiceProfileView> = self
            .org_json(org, |h, u| {
                h.get(format!("{u}/api/v1/voice-id/profiles?limit=100"))
            })
            .await?;
        let proof = self_proof(&self.email);
        let profile = page.results.into_iter().find(|p| p.consent_proof == proof);
        let model = self
            .models_of(org, AsrTask::SpeakerEmbedding)
            .await?
            .into_iter()
            .find(usable)
            .map(|m| m.id);
        Ok(VoiceSetup {
            profile,
            model,
            can_enrol,
        })
    }

    /// Enrols the user's voice from `wav`: creates the Person object and the
    /// profile on first use (the user gives the consent in the window), else
    /// replaces the profile's clips; then generates the voice profile.
    pub async fn enrol_voice(
        &mut self,
        org: i64,
        model: &str,
        profile: Option<VoiceProfileView>,
        wav: Vec<u8>,
    ) -> Result<VoiceProfileView, ApiError> {
        let id = match profile {
            Some(p) => {
                for s in &p.samples {
                    self.org_send(org, |h, u| {
                        h.delete(format!(
                            "{u}/api/v1/voice-id/profiles/{}/samples/{}",
                            p.id, s.id
                        ))
                    })
                    .await?;
                }
                p.id
            }
            None => {
                let title = self.email.clone();
                let person: Value = self
                    .org_json(org, |h, u| {
                        h.post(format!("{u}/api/v1/ontology/objects"))
                            .json(&json!({"type_key": PERSON_TYPE, "title": title}))
                    })
                    .await?;
                let object_id = person["id"].as_str().ok_or(ApiError::NotPnex)?.to_owned();
                let input = VoiceProfileInput {
                    object_id,
                    model_id: model.to_owned(),
                    consent_at: chrono::Local::now().format("%Y-%m-%d").to_string(),
                    consent_scope: "Recognise my own voice in my PNeX recordings, until I \
                                    withdraw it."
                        .into(),
                    consent_proof: self_proof(&self.email),
                    consent_confirmed: true,
                };
                let created: VoiceProfileView = self
                    .org_json(org, |h, u| {
                        h.post(format!("{u}/api/v1/voice-id/profiles")).json(&input)
                    })
                    .await?;
                created.id
            }
        };
        self.org_send(org, |h, u| {
            h.post(format!(
                "{u}/api/v1/voice-id/profiles/{id}/samples?name=my-voice.wav&content_type=audio/wav"
            ))
            .header("Content-Type", "audio/wav")
            .timeout(std::time::Duration::from_secs(120))
            .body(wav)
        })
        .await?;
        self.org_json(org, |h, u| {
            h.post(format!("{u}/api/v1/voice-id/profiles/{id}/generate"))
                .timeout(std::time::Duration::from_secs(300))
        })
        .await
    }

    /// Compares `wav` with the user's voice profile; nothing is stored.
    pub async fn test_voice(
        &mut self,
        org: i64,
        object_id: &str,
        wav: Vec<u8>,
    ) -> Result<VoiceTestResult, ApiError> {
        self.org_json(org, |h, u| {
            h.post(format!(
                "{u}/api/v1/voice-id/test?name=clip.wav&content_type=audio/wav&object_id={object_id}"
            ))
            .header("Content-Type", "audio/wav")
            .timeout(std::time::Duration::from_secs(120))
            .body(wav)
        })
        .await
    }
}

/// Only models that passed their check can be tested (409 otherwise).
pub fn usable(model: &AsrModel) -> bool {
    model.check.status == ModelCheckStatus::Valid
}

/// Opens the server's SSO page in the system browser and waits (ten minutes
/// at most) for the code the server's native bridge relays, then exchanges it
/// with the PKCE verifier, which never leaves this process.
async fn browser_login(http: &reqwest::Client, url: &str) -> Result<TokenResponse, ApiError> {
    use base64::engine::general_purpose::URL_SAFE_NO_PAD;
    use base64::Engine;
    use sha2::{Digest, Sha256};

    let mut bytes = [0u8; 32];
    getrandom::getrandom(&mut bytes).map_err(|_| ApiError::Unreachable)?;
    let verifier = URL_SAFE_NO_PAD.encode(bytes);
    let challenge = URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()));
    let redirect = format!("{url}/api/v1/oauth2/native");
    let sso = reqwest::Url::parse_with_params(
        &format!("{url}/api/v1/oauth2/sso"),
        [
            ("code_challenge", challenge.as_str()),
            ("code_challenge_method", "S256"),
            ("redirect_uri", redirect.as_str()),
            // The challenge is unique per flow: it is also the bridge key.
            ("state", challenge.as_str()),
        ],
    )
    .map_err(|_| ApiError::Unreachable)?;
    // A failed open is not fatal: the user can still be slow or retry.
    if let Err(e) = webbrowser::open(sso.as_str()) {
        eprintln!("browser opening failed: {e}");
    }
    for _ in 0..1200 {
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
        let Ok(res) = http
            .get(format!("{url}/api/v1/oauth2/native/{challenge}"))
            .send()
            .await
        else {
            continue;
        };
        let poll: Value = res.json().await.unwrap_or(Value::Null);
        if let Some(code) = poll["code"].as_str() {
            let body = json!({
                "grant_type": "authorization_code",
                "code": code,
                "code_verifier": verifier,
                "redirect_uri": redirect,
            });
            return token(http, url, &body).await;
        }
    }
    Err(ApiError::LoginTimedOut)
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
