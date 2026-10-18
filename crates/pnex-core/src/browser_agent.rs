//! Browser agents (extension-collector.md v2, D254–D269): pairing,
//! missions and the `/ws/extension` protocol, shared by the backend and the
//! wasm extension. Pure, wasm-safe.
//!
//! - pairing: [`PairRequest`] signed by the install key (D266), answered by
//!   [`PairResponse`] (device token + Noise key);
//! - tickets: [`TicketRequest`] signed by the install key, traded for a
//!   one-time `/ws/extension` ticket;
//! - missions: [`MissionSpec`] (typed, D256), checked by [`check_spec`];
//! - link: [`ServerMsg`] / [`AgentMsg`], JSON sealed in Noise frames.

use serde::{Deserialize, Serialize};

use crate::err_codes;

/// Shortest refresh of a mission (D258 floor, also compiled in the agent).
pub const MIN_INTERVAL_SECS: i32 = 60;
pub const MAX_INTERVAL_SECS: i32 = 86_400;
pub const NAME_MAX: usize = 120;
pub const URL_MAX: usize = 2048;
pub const SELECTOR_MAX: usize = 512;
pub const METRIC_MAX: usize = 64;
/// Max length of a fingerprint field.
pub const FINGERPRINT_FIELD_MAX: usize = 120;
/// Accepted clock skew of a signed request, in milliseconds.
pub const SIGNED_SKEW_MS: i64 = 120_000;
/// Longest value a `collect_value` report may carry.
pub const VALUE_MAX: usize = 256;

/// Message the install key signs to pair with an org (D266): proof that
/// the caller holds the private key, bound to the org and a timestamp.
pub fn pair_message(org_id: i64, ts_ms: i64) -> String {
    format!("pnex-ext-pair\n{org_id}\n{ts_ms}")
}

/// Message the install key signs to obtain a websocket ticket.
pub fn ticket_message(device_id: &str, ts_ms: i64) -> String {
    format!("pnex-ext-ticket\n{device_id}\n{ts_ms}")
}

/// Message the install key signs to upload a capture (D262): bound to the
/// mission and to the SHA-256 (hex) of the body. Sent as the `X-Pnex-Ts`
/// and `X-Pnex-Signature` headers.
pub fn upload_message(device_id: &str, ts_ms: i64, mission_id: &str, body_sha256: &str) -> String {
    format!("pnex-ext-upload\n{device_id}\n{ts_ms}\n{mission_id}\n{body_sha256}")
}

/// Largest readable text of a page snapshot (bytes).
pub const PAGE_TEXT_MAX: usize = 1024 * 1024;
/// What a `collect_page` mission uploads (D261): the readable text only,
/// stored as a Markdown document (title, source, text) and indexed by the
/// document search. No HTML leaves the page.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct PageSnapshot {
    pub mission_id: String,
    /// Address after redirects.
    pub url: String,
    #[serde(default)]
    pub title: String,
    /// Readable text (what is indexed and versioned).
    pub text: String,
}

/// Answer of a page upload.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct PageStored {
    /// Media document of the mission.
    #[cfg_attr(feature = "openapi", schema(format = "uuid"))]
    pub asset_id: String,
    /// Current version number of the document.
    #[cfg_attr(feature = "openapi", schema(format = Int64))]
    pub version: i64,
    /// `false` when the text did not change (no new version).
    pub changed: bool,
}

/// What the extension reads by itself at pairing (no user input, D266).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct AgentFingerprint {
    #[serde(default)]
    pub browser: String,
    #[serde(default)]
    pub browser_version: String,
    #[serde(default)]
    pub os: String,
    #[serde(default)]
    pub arch: String,
    #[serde(default)]
    pub language: String,
    #[serde(default)]
    pub timezone: String,
    #[serde(default)]
    pub screen: String,
    #[serde(default)]
    pub extension_version: String,
}

impl AgentFingerprint {
    /// Bounded copy (each field cut to [`FINGERPRINT_FIELD_MAX`] chars).
    pub fn bounded(&self) -> Self {
        let cut = |s: &str| s.trim().chars().take(FINGERPRINT_FIELD_MAX).collect();
        Self {
            browser: cut(&self.browser),
            browser_version: cut(&self.browser_version),
            os: cut(&self.os),
            arch: cut(&self.arch),
            language: cut(&self.language),
            timezone: cut(&self.timezone),
            screen: cut(&self.screen),
            extension_version: cut(&self.extension_version),
        }
    }

    /// Default agent name: "Chrome · Linux · Europe/Paris".
    pub fn default_name(&self) -> String {
        let parts: Vec<&str> = [&self.browser, &self.os, &self.timezone]
            .into_iter()
            .map(|s| s.trim())
            .filter(|s| !s.is_empty())
            .collect();
        if parts.is_empty() {
            "Browser agent".into()
        } else {
            parts.join(" · ").chars().take(NAME_MAX).collect()
        }
    }
}

/// Pairing body, sent with the user's bearer token (the only user gesture).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct PairRequest {
    /// Raw uncompressed P-256 public key (65 bytes), base64url without padding.
    pub install_key: String,
    #[serde(default)]
    pub fingerprint: AgentFingerprint,
    /// Unix milliseconds of the signature.
    pub ts: i64,
    /// ECDSA P-256 SHA-256 signature (64 bytes r‖s, base64url) of
    /// [`pair_message`].
    pub signature: String,
}

/// A browser agent of the org (a device of the `browser_agent` model).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct BrowserAgent {
    /// Device registry id.
    #[cfg_attr(feature = "openapi", schema(format = Int64))]
    pub id: i64,
    pub device_id: String,
    pub name: String,
    pub fingerprint: AgentFingerprint,
    pub online: bool,
    #[cfg_attr(feature = "openapi", schema(format = DateTime))]
    pub created_at: String,
    #[cfg_attr(feature = "openapi", schema(format = DateTime))]
    pub updated_at: String,
}

/// Pairing answer: the device credentials, shown once.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct PairResponse {
    pub agent: BrowserAgent,
    /// Device token (bearer of the ticket request).
    pub token: String,
    /// Noise pre-shared key of `/ws/extension` (base64, 32 bytes).
    pub encryption_key: String,
}

/// Partial update of an agent.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct BrowserAgentPatch {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

/// Ticket body, sent with the device token.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct TicketRequest {
    pub ts: i64,
    /// Signature of [`ticket_message`] (64 bytes r‖s, base64url).
    pub signature: String,
}

/// What `collect_value` reads from the matched element.
pub const ATTR_TEXT: &str = "text";

/// `collect_value` (v1 school): refresh a page, read one value.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct CollectValueParams {
    /// http(s) page, without credentials.
    pub url: String,
    /// CSS selector of the element (the "pin", D48).
    pub selector: String,
    /// `text` (default) or `attr:<name>`.
    #[serde(default = "default_attr")]
    pub attr: String,
    /// Series name `[a-z][a-z0-9_]{0,63}`.
    pub metric: String,
}

fn default_attr() -> String {
    ATTR_TEXT.into()
}

/// `collect_page` (D261): refresh a page, keep its readable text (and the
/// cleaned HTML) as a versioned document.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct CollectPageParams {
    /// http(s) page, without credentials.
    pub url: String,
}

/// Social platform an agent follows accounts on (D254). TikTok comes with
/// its adapter (lot L3).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "snake_case")]
pub enum SocialPlatform {
    X,
}

impl SocialPlatform {
    pub fn wire(self) -> &'static str {
        match self {
            Self::X => "x",
        }
    }

    /// Profile page of an account.
    pub fn profile_url(self, handle: &str) -> String {
        match self {
            Self::X => format!("https://x.com/{handle}"),
        }
    }
}

/// What a `follow_account` mission does with a new video (D267).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "snake_case")]
pub enum MediaPolicy {
    /// Download unless the platform gave captions. Until captions are read
    /// (D281, after D211), behaves as `always`.
    #[default]
    IfNoTranscript,
    Always,
    /// Registry and metadata only.
    MetadataOnly,
}

/// `follow_account` (D256): the new posts of one public account.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct FollowAccountParams {
    pub platform: SocialPlatform,
    /// Account handle, without `@`.
    pub handle: String,
    #[serde(default)]
    pub media_policy: MediaPolicy,
}

/// Longest account handle.
pub const HANDLE_MAX: usize = 50;

/// A typed mission (D256). `watch_live` arrives with its lot.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(tag = "kind", content = "params", rename_all = "snake_case")]
pub enum MissionSpec {
    CollectValue(CollectValueParams),
    CollectPage(CollectPageParams),
    FollowAccount(FollowAccountParams),
}

impl MissionSpec {
    pub fn kind(&self) -> &'static str {
        match self {
            Self::CollectValue(_) => "collect_value",
            Self::CollectPage(_) => "collect_page",
            Self::FollowAccount(_) => "follow_account",
        }
    }
}

/// Run state of a mission, reported by its agent.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "snake_case")]
pub enum MissionState {
    #[default]
    Idle,
    Running,
    Blocked,
    Failed,
}

impl MissionState {
    pub fn wire(self) -> &'static str {
        match self {
            Self::Idle => "idle",
            Self::Running => "running",
            Self::Blocked => "blocked",
            Self::Failed => "failed",
        }
    }

    pub fn parse(raw: &str) -> Option<Self> {
        serde_json::from_value(serde_json::Value::String(raw.into())).ok()
    }
}

/// Codes an agent may attach to a mission state (resolved by the UI as
/// `err-<code>`); any other code is stored empty.
pub const AGENT_STATE_CODES: &[&str] = &[
    err_codes::EXT_SELECTOR_NOT_FOUND,
    err_codes::EXT_VALUE_NOT_NUMERIC,
    err_codes::EXT_LOGIN_REQUIRED,
    err_codes::EXT_CHALLENGE,
    err_codes::EXT_RATE_LIMITED,
    err_codes::EXT_TAB_HIDDEN,
    err_codes::EXT_PERMISSION_MISSING,
];

/// A mission of the org.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct Mission {
    #[cfg_attr(feature = "openapi", schema(format = "uuid"))]
    pub id: String,
    pub name: String,
    pub spec: MissionSpec,
    /// Normalized target, key of the lease (D257).
    pub target: String,
    pub interval_secs: i32,
    /// Pinned agent (device registry id); none = any online agent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "openapi", schema(format = Int64))]
    pub agent_id: Option<i64>,
    pub enabled: bool,
    pub state: MissionState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub state_code: Option<String>,
    pub version: i32,
    #[cfg_attr(feature = "openapi", schema(format = DateTime))]
    pub created_at: String,
    #[cfg_attr(feature = "openapi", schema(format = DateTime))]
    pub updated_at: String,
}

/// Create body of a mission.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct MissionCreate {
    pub name: String,
    pub spec: MissionSpec,
    pub interval_secs: i32,
    #[serde(default)]
    #[cfg_attr(feature = "openapi", schema(format = Int64))]
    pub agent_id: Option<i64>,
    #[serde(default = "yes")]
    pub enabled: bool,
}

fn yes() -> bool {
    true
}

/// Partial update of a mission, on the version the caller read.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct MissionPatch {
    pub expected_version: i32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub spec: Option<MissionSpec>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub interval_secs: Option<i32>,
    /// `Some(None)` unpins the mission.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "openapi", schema(value_type = Option<i64>, format = Int64))]
    pub agent_id: Option<Option<i64>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enabled: Option<bool>,
}

type FieldError = (String, String);

fn field(f: &str, token: impl Into<String>) -> FieldError {
    (f.into(), token.into())
}

fn too_long(max: usize) -> String {
    format!("{}:{max}", err_codes::FIELD_MAX_LENGTH)
}

/// Trimmed, non-empty, bounded mission or agent name.
pub fn check_name(raw: &str) -> Result<String, FieldError> {
    let name = raw.trim();
    if name.is_empty() {
        return Err(field("name", err_codes::FIELD_REQUIRED));
    }
    if name.chars().count() > NAME_MAX {
        return Err(field("name", too_long(NAME_MAX)));
    }
    Ok(name.into())
}

pub fn check_interval(secs: i32) -> Result<i32, FieldError> {
    if (MIN_INTERVAL_SECS..=MAX_INTERVAL_SECS).contains(&secs) {
        Ok(secs)
    } else {
        Err(field("interval_secs", err_codes::FIELD_INVALID))
    }
}

/// Validates and normalizes a spec; returns it with its target (D257).
pub fn check_spec(spec: &MissionSpec) -> Result<(MissionSpec, String), FieldError> {
    match spec {
        MissionSpec::CollectValue(p) => {
            let (url, target) = check_page_url(&p.url, "spec.params.url")?;
            let selector = p.selector.trim();
            if selector.is_empty() {
                return Err(field("spec.params.selector", err_codes::FIELD_REQUIRED));
            }
            if selector.chars().count() > SELECTOR_MAX {
                return Err(field("spec.params.selector", too_long(SELECTOR_MAX)));
            }
            let attr = p.attr.trim();
            let attr_ok = attr == ATTR_TEXT
                || attr.strip_prefix("attr:").is_some_and(|n| {
                    !n.is_empty()
                        && n.len() <= 64
                        && n.chars()
                            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == ':')
                });
            if !attr_ok {
                return Err(field("spec.params.attr", err_codes::FIELD_INVALID));
            }
            if !is_metric_name(p.metric.trim()) {
                return Err(field("spec.params.metric", err_codes::FIELD_INVALID));
            }
            Ok((
                MissionSpec::CollectValue(CollectValueParams {
                    url,
                    selector: selector.into(),
                    attr: attr.into(),
                    metric: p.metric.trim().into(),
                }),
                target,
            ))
        }
        MissionSpec::CollectPage(p) => {
            let (url, target) = check_page_url(&p.url, "spec.params.url")?;
            Ok((MissionSpec::CollectPage(CollectPageParams { url }), target))
        }
        MissionSpec::FollowAccount(p) => {
            let handle = p.handle.trim().trim_start_matches('@');
            let valid = !handle.is_empty()
                && handle.len() <= HANDLE_MAX
                && handle
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '.');
            if !valid {
                return Err(field("spec.params.handle", err_codes::FIELD_INVALID));
            }
            let target = format!("{}:@{}", p.platform.wire(), handle.to_ascii_lowercase());
            Ok((
                MissionSpec::FollowAccount(FollowAccountParams {
                    platform: p.platform,
                    handle: handle.into(),
                    media_policy: p.media_policy,
                }),
                target,
            ))
        }
    }
}

/// `[a-z][a-z0-9_]{0,63}`.
pub fn is_metric_name(s: &str) -> bool {
    let mut chars = s.chars();
    chars.next().is_some_and(|c| c.is_ascii_lowercase())
        && s.len() <= METRIC_MAX
        && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
}

/// http(s) URL without credentials, fragment dropped; target
/// `web:<host><path>` (one lease per page).
fn check_page_url(raw: &str, f: &str) -> Result<(String, String), FieldError> {
    let raw = raw.trim();
    if raw.is_empty() {
        return Err(field(f, err_codes::FIELD_REQUIRED));
    }
    if raw.len() > URL_MAX {
        return Err(field(f, too_long(URL_MAX)));
    }
    let mut url = url::Url::parse(raw).map_err(|_| field(f, err_codes::FIELD_INVALID))?;
    let host = url.host_str().map(str::to_ascii_lowercase);
    match (url.scheme(), host) {
        ("http" | "https", Some(host)) if url.username().is_empty() && url.password().is_none() => {
            url.set_fragment(None);
            let target = format!("web:{host}{}", url.path());
            Ok((url.into(), target))
        }
        _ => Err(field(f, err_codes::FIELD_INVALID)),
    }
}

/// Platform of a target: the tab it runs in (D255, one tab per platform).
pub fn platform_of(target: &str) -> &str {
    match target.strip_prefix("web:") {
        Some(rest) => {
            let host_len = rest.find('/').unwrap_or(rest.len());
            &target[..4 + host_len]
        }
        None => target.split(':').next().unwrap_or(target),
    }
}

/// A mission as the agent receives it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AssignedMission {
    pub id: String,
    pub spec: MissionSpec,
    pub target: String,
    pub interval_secs: i32,
}

/// Server → agent (desired state, D262).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "t", rename_all = "snake_case")]
pub enum ServerMsg {
    /// The full set of missions this agent holds the lease of; sent at
    /// connection and on every change.
    Missions {
        missions: Vec<AssignedMission>,
    },
    /// Answer to [`AgentMsg::Seen`]: the posts whose video the agent should
    /// download and upload (new in the registry, policy allows it, D259).
    Capture {
        mission_id: String,
        platform_ids: Vec<String>,
    },
    Pong,
}

/// Agent → server.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "t", rename_all = "snake_case")]
pub enum AgentMsg {
    /// A value read by a `collect_value` mission.
    Report {
        mission_id: String,
        value: String,
    },
    /// Run state of a mission, with an optional code of
    /// [`AGENT_STATE_CODES`].
    State {
        mission_id: String,
        state: MissionState,
        #[serde(default)]
        code: Option<String>,
    },
    /// Posts a `follow_account` mission saw on the account (at most
    /// [`SEEN_MAX`] per message).
    Seen {
        mission_id: String,
        posts: Vec<SeenPost>,
    },
    Ping,
}

/// Posts per [`AgentMsg::Seen`].
pub const SEEN_MAX: usize = 100;
/// Largest post text kept.
pub const POST_TEXT_MAX: usize = 4000;

/// A post as an adapter reads it from the platform's own responses.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SeenPost {
    /// Id of the post on the platform.
    pub platform_id: String,
    pub url: String,
    /// Handle of the account that posted it.
    pub author: String,
    #[serde(default)]
    pub text: String,
    /// RFC 3339.
    #[serde(default)]
    pub published_at: Option<String>,
    /// Best progressive (mp4) variant, when the post carries a video.
    #[serde(default)]
    pub video_url: Option<String>,
    #[serde(default)]
    pub duration_ms: Option<i64>,
}

/// Message the install key signs to upload a video (D262): bound to the
/// mission, the post and the SHA-256 (hex) of the file.
pub fn video_upload_message(
    device_id: &str,
    ts_ms: i64,
    mission_id: &str,
    platform_id: &str,
    body_sha256: &str,
) -> String {
    format!("pnex-ext-video\n{device_id}\n{ts_ms}\n{mission_id}\n{platform_id}\n{body_sha256}")
}

/// Largest captured video (bytes).
pub const VIDEO_MAX: usize = 200 * 1024 * 1024;

/// State of a video in the registry (D259).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "snake_case")]
pub enum VideoState {
    /// Known, not downloaded (metadata only, or capture pending/failed).
    Seen,
    /// Downloaded into the media library.
    Captured,
}

/// A row of the video registry.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct SocialVideo {
    pub platform: SocialPlatform,
    pub platform_id: String,
    pub url: String,
    pub author: String,
    pub text: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "openapi", schema(format = DateTime))]
    pub published_at: Option<String>,
    pub has_video: bool,
    pub state: VideoState,
    /// Media of the captured video.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "openapi", schema(format = "uuid"))]
    pub media_asset_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "openapi", schema(format = "uuid"))]
    pub mission_id: Option<String>,
    #[cfg_attr(feature = "openapi", schema(format = DateTime))]
    pub first_seen_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "openapi", schema(format = DateTime))]
    pub captured_at: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn value(url: &str, selector: &str, attr: &str, metric: &str) -> MissionSpec {
        MissionSpec::CollectValue(CollectValueParams {
            url: url.into(),
            selector: selector.into(),
            attr: attr.into(),
            metric: metric.into(),
        })
    }

    #[test]
    fn collect_value_is_normalized_with_its_target() {
        let (spec, target) = check_spec(&value(
            " https://Example.org/a/b?x=1#frag ",
            " .price ",
            "text",
            "price",
        ))
        .unwrap();
        assert_eq!(target, "web:example.org/a/b");
        assert_eq!(platform_of(&target), "web:example.org");
        let MissionSpec::CollectValue(p) = spec else {
            panic!("collect_value expected");
        };
        assert_eq!(p.url, "https://example.org/a/b?x=1");
        assert_eq!(p.selector, ".price");
    }

    #[test]
    fn collect_value_rejects_bad_fields() {
        let bad = |s: MissionSpec| check_spec(&s).unwrap_err().0;
        assert_eq!(
            bad(value("ftp://x.org/", ".a", "text", "m")),
            "spec.params.url"
        );
        assert_eq!(
            bad(value("https://u:p@x.org/", ".a", "text", "m")),
            "spec.params.url"
        );
        assert_eq!(
            bad(value("https://x.org/", " ", "text", "m")),
            "spec.params.selector"
        );
        assert_eq!(
            bad(value("https://x.org/", ".a", "html", "m")),
            "spec.params.attr"
        );
        assert_eq!(
            bad(value("https://x.org/", ".a", "text", "9m")),
            "spec.params.metric"
        );
        assert!(check_spec(&value("https://x.org/", ".a", "attr:data-v", "m_1")).is_ok());
    }

    #[test]
    fn follow_account_target() {
        let spec = MissionSpec::FollowAccount(FollowAccountParams {
            platform: SocialPlatform::X,
            handle: " @Some_Account ".into(),
            media_policy: MediaPolicy::Always,
        });
        let (spec, target) = check_spec(&spec).unwrap();
        assert_eq!(target, "x:@some_account");
        assert_eq!(platform_of(&target), "x");
        let MissionSpec::FollowAccount(p) = spec else {
            panic!("follow_account expected");
        };
        assert_eq!(p.handle, "Some_Account");
        let bad = MissionSpec::FollowAccount(FollowAccountParams {
            platform: SocialPlatform::X,
            handle: "a/b".into(),
            media_policy: MediaPolicy::default(),
        });
        assert_eq!(check_spec(&bad).unwrap_err().0, "spec.params.handle");
        let json =
            serde_json::json!({"kind":"follow_account","params":{"platform":"x","handle":"a"}});
        let parsed: MissionSpec = serde_json::from_value(json).unwrap();
        assert!(matches!(
            parsed,
            MissionSpec::FollowAccount(FollowAccountParams {
                media_policy: MediaPolicy::IfNoTranscript,
                ..
            })
        ));
    }

    #[test]
    fn interval_floor_and_wire_shapes() {
        assert!(check_interval(MIN_INTERVAL_SECS - 1).is_err());
        assert!(check_interval(MIN_INTERVAL_SECS).is_ok());
        let msg = serde_json::to_value(AgentMsg::State {
            mission_id: "m".into(),
            state: MissionState::Blocked,
            code: Some(err_codes::EXT_LOGIN_REQUIRED.into()),
        })
        .unwrap();
        assert_eq!(msg["t"], "state");
        assert_eq!(msg["state"], "blocked");
        let spec = serde_json::to_value(value("https://x.org/", ".a", "text", "m")).unwrap();
        assert_eq!(spec["kind"], "collect_value");
        assert_eq!(spec["params"]["attr"], "text");
    }

    #[test]
    fn default_name_skips_empty_parts() {
        let fp = AgentFingerprint {
            browser: "Chrome".into(),
            os: "Linux".into(),
            ..Default::default()
        };
        assert_eq!(fp.default_name(), "Chrome · Linux");
        assert_eq!(AgentFingerprint::default().default_name(), "Browser agent");
    }
}
