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
    /// `shared` or `dedicated:<platform>` (D269).
    pub mode: String,
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
    /// `shared` or `dedicated:<platform>` (D269).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mode: Option<String>,
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
    #[serde(rename = "youtube")]
    YouTube,
    #[serde(rename = "tiktok")]
    TikTok,
}

impl SocialPlatform {
    pub fn wire(self) -> &'static str {
        match self {
            Self::X => "x",
            Self::YouTube => "youtube",
            Self::TikTok => "tiktok",
        }
    }

    pub fn parse(raw: &str) -> Option<Self> {
        match raw {
            "x" => Some(Self::X),
            "youtube" => Some(Self::YouTube),
            "tiktok" => Some(Self::TikTok),
            _ => None,
        }
    }

    /// Pages of an account the agent reads, in order (D283).
    pub fn list_urls(self, handle: &str) -> Vec<String> {
        match self {
            Self::X => vec![format!("https://x.com/{handle}")],
            Self::YouTube => vec![
                format!("https://www.youtube.com/@{handle}/videos"),
                format!("https://www.youtube.com/@{handle}/shorts"),
            ],
            Self::TikTok => vec![format!("https://www.tiktok.com/@{handle}")],
        }
    }

    /// The platform pauses its videos in a hidden tab (D269): its tab is
    /// brought to the front while the agent reads it.
    pub fn needs_foreground(self) -> bool {
        matches!(self, Self::TikTok)
    }

    /// Sites the agent must be allowed on: the pages, plus where the files
    /// come from.
    pub fn sites(self) -> &'static [&'static str] {
        match self {
            Self::X => &["https://x.com/", "https://video.twimg.com/"],
            Self::YouTube => &["https://www.youtube.com/"],
            // Play addresses and captions live on subdomains of tiktok.com.
            Self::TikTok => &["https://www.tiktok.com/", "https://*.tiktok.com/"],
        }
    }
}

/// `follow_account` (D256): the new posts of one public account.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct FollowAccountParams {
    pub platform: SocialPlatform,
    /// Account handle, without `@`.
    pub handle: String,
    /// Download or record every video (D284). Off: metadata, text and the
    /// platform's captions when it has some, no file.
    #[serde(default)]
    pub force_video: bool,
    /// Ontology object (person or organization) behind the account: each
    /// captured media is linked to it by `published_by` (D278, D281).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "openapi", schema(format = "uuid"))]
    pub publisher_id: Option<String>,
}

/// Mode of an agent (D269): `shared` runs every platform but the
/// exclusive ones; `dedicated:<platform>` runs that platform only.
pub const MODE_SHARED: &str = "shared";

/// The platform pauses in a hidden tab: only a dedicated agent runs it.
pub fn is_exclusive(platform: &str) -> bool {
    SocialPlatform::parse(platform).is_some_and(SocialPlatform::needs_foreground)
}

/// Whether an agent in `mode` reads `platform` in a tab at the front (D269).
pub fn runs_in_front(mode: &str, platform: &str) -> bool {
    is_exclusive(platform) || mode.starts_with("dedicated:")
}

/// Whether an agent in `mode` runs a mission on `platform` (D269).
pub fn mode_allows(mode: &str, platform: &str) -> bool {
    match mode.strip_prefix("dedicated:") {
        Some(dedicated) => platform == dedicated,
        None => !is_exclusive(platform),
    }
}

/// `shared` or `dedicated:<social platform>`.
pub fn check_mode(raw: &str) -> Result<String, (String, String)> {
    let mode = raw.trim();
    let ok = mode == MODE_SHARED
        || mode
            .strip_prefix("dedicated:")
            .is_some_and(|p| SocialPlatform::parse(p).is_some());
    if ok {
        Ok(mode.into())
    } else {
        Err(("mode".into(), err_codes::FIELD_INVALID.into()))
    }
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
    WatchLive(WatchLiveParams),
    CrawlSite(CrawlSiteParams),
    /// Created by the server only (D308): one proof of one page.
    CaptureProof(CaptureProofParams),
}

/// `capture_proof` (D308, D310): capture one page as a proof.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct CaptureProofParams {
    pub url: String,
    /// Registry page the proof is for.
    #[cfg_attr(feature = "openapi", schema(format = "uuid"))]
    pub page_id: String,
}

/// The parts of a proof (D310), uploaded one by one, signed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "snake_case")]
pub enum ProofPart {
    /// Full-page PNG (scroll + visible-tab captures, stitched).
    Screenshot,
    /// Rendered DOM, scripts, frames and handlers removed: download only.
    Html,
    /// A [`PageSnapshot`] (url after redirects, title, readable text).
    Text,
}

impl ProofPart {
    pub fn wire(self) -> &'static str {
        match self {
            Self::Screenshot => "screenshot",
            Self::Html => "html",
            Self::Text => "text",
        }
    }

    pub fn parse(raw: &str) -> Option<Self> {
        serde_json::from_value(serde_json::Value::String(raw.into())).ok()
    }

    pub const ALL: [ProofPart; 3] = [Self::Screenshot, Self::Html, Self::Text];
}

/// Tallest full-page capture, CSS pixels (D310).
pub const PROOF_HEIGHT_MAX: u32 = 20_000;
/// Largest screenshot part, bytes.
pub const PROOF_SCREENSHOT_MAX: usize = 25 * 1024 * 1024;
/// Largest HTML part, bytes.
pub const PROOF_HTML_MAX: usize = 5 * 1024 * 1024;

/// Message the install key signs to upload a part of a proof.
pub fn proof_upload_message(
    device_id: &str,
    ts_ms: i64,
    mission_id: &str,
    part: ProofPart,
    body_sha256: &str,
) -> String {
    format!(
        "pnex-ext-proof\n{device_id}\n{ts_ms}\n{mission_id}\n{}\n{body_sha256}",
        part.wire()
    )
}

/// Answer of a proof part upload: the proof, sealed once its parts are in.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct ProofPartStored {
    #[cfg_attr(feature = "openapi", schema(format = "uuid"))]
    pub proof_id: String,
    pub sealed: bool,
}

/// A web proof (D310): its parts and, once sealed, its manifest and the
/// place it holds in the org's chain of manifests.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct WebProof {
    #[cfg_attr(feature = "openapi", schema(format = "uuid"))]
    pub id: String,
    #[cfg_attr(feature = "openapi", schema(format = Int64))]
    pub seq: i64,
    pub url: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "openapi", schema(format = "uuid"))]
    pub site_page_id: Option<String>,
    /// `open` (parts arriving) or `sealed`.
    pub state: String,
    /// `{part: {media_id, sha256, bytes}}`.
    #[cfg_attr(feature = "openapi", schema(value_type = Object))]
    pub parts: serde_json::Value,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "openapi", schema(value_type = Option<Object>))]
    pub manifest: Option<serde_json::Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub manifest_sha256: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "openapi", schema(format = DateTime))]
    pub captured_at: Option<String>,
}

/// `GET …/proofs/verify`: the org's chain of manifests recomputed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct ProofChainCheck {
    pub ok: bool,
    /// Sealed proofs checked.
    #[cfg_attr(feature = "openapi", schema(format = Int64))]
    pub count: i64,
    /// First proof whose manifest, parts or link to the previous one does
    /// not hold.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "openapi", schema(format = Int64))]
    pub broken_at: Option<i64>,
}

/// A page a browser agent read while crawling a site (D307, agent
/// executor), sent signed like a page capture.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct SitePageReport {
    pub mission_id: String,
    pub url: String,
    #[serde(default)]
    pub title: String,
    pub text: String,
    #[serde(default)]
    pub depth: u32,
}

/// End of an agent crawl: the pages it read; a complete walk counts the
/// missing ones (`gone` after two).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct CrawlDone {
    pub mission_id: String,
    pub urls: Vec<String>,
    pub complete: bool,
}

/// A page of the site registry (D309).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct SitePage {
    #[cfg_attr(feature = "openapi", schema(format = "uuid"))]
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "openapi", schema(format = "uuid"))]
    pub mission_id: Option<String>,
    pub site: String,
    pub url: String,
    pub title: String,
    pub depth: u32,
    /// `listed` | `relevant` | `captured` | `gone` | `blocked`.
    pub state: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub state_code: Option<String>,
    #[cfg_attr(feature = "openapi", schema(format = DateTime))]
    pub first_seen_at: String,
    #[cfg_attr(feature = "openapi", schema(format = DateTime))]
    pub last_seen_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text_hash: Option<String>,
    /// Crawls in a row without the page.
    pub misses: i32,
    /// Document holding the versions of its text (relevant pages).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "openapi", schema(format = "uuid"))]
    pub doc_asset_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "openapi", schema(format = "uuid"))]
    pub last_proof_id: Option<String>,
    /// Object written by the mission's `record`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "openapi", schema(format = "uuid"))]
    pub object_id: Option<String>,
}

/// Who walks a site (D307): the server (public pages, no script run) or a
/// browser agent (pages rendered by JavaScript, the D258 pace).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "snake_case")]
pub enum CrawlExecutor {
    #[default]
    Server,
    Agent,
}

/// How a relevance rule reads the page text (closed list, D307).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "snake_case")]
pub enum MatchKind {
    /// A word, case and accents ignored.
    Word,
    /// A phrase, case and accents ignored.
    Phrase,
    /// A bounded regular expression (size-limited, no backreference).
    Regex,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct Matcher {
    pub kind: MatchKind,
    pub value: String,
}

/// Registry fields a crawl may write on objects (D309).
pub const RECORD_FIELDS: [&str; 7] = [
    "url",
    "title",
    "first_seen",
    "last_seen",
    "text_hash",
    "relevant",
    "state",
];

/// Relevant pages written as objects of an org type: registry field →
/// property key (`url` → the type's unique property). Optional link of
/// each page object to one object (its site).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct PageRecord {
    #[serde(rename = "type")]
    pub type_key: String,
    pub properties: std::collections::BTreeMap<String, String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub link_type: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "openapi", schema(format = "uuid"))]
    pub link_to: Option<String>,
    /// Link type from the page object to each part of its proofs (D310),
    /// with `captured_at`, `sha256_manifest` and `kind` (`capture`, `html`,
    /// `text`) when the link type declares them.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub proof_link: Option<String>,
}

/// `crawl_site` (D307): walk one declared site, list its pages, keep the
/// relevant ones (and their proofs, D308).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct CrawlSiteParams {
    /// Any address of the site: its origin bounds the walk.
    pub url: String,
    #[serde(default)]
    pub executor: CrawlExecutor,
    /// Pages per crawl (default 200, at most 2,000).
    #[serde(default = "crawl_pages_default")]
    pub max_pages: u32,
    /// Link depth from the entry points (default and at most 5).
    #[serde(default = "crawl_depth_default")]
    pub max_depth: u32,
    /// Path prefixes never visited (`/blog/`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub exclude: Vec<String>,
    /// A page matching any rule is relevant; none = every page is.
    #[serde(default, rename = "match", skip_serializing_if = "Vec::is_empty")]
    pub matchers: Vec<Matcher>,
    /// Relevant pages new or changed get a proof by an agent (D308).
    #[serde(default = "yes")]
    pub proofs: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub record: Option<PageRecord>,
}

pub const CRAWL_PAGES_DEFAULT: u32 = 200;
pub const CRAWL_PAGES_MAX: u32 = 2_000;
pub const CRAWL_DEPTH_MAX: u32 = 5;
/// Server crawl floor between two requests to one host (compiled, D307).
pub const CRAWL_DELAY_MS: u64 = 2_000;
pub const MATCHERS_MAX: usize = 50;
pub const MATCH_VALUE_MAX: usize = 200;

fn crawl_pages_default() -> u32 {
    CRAWL_PAGES_DEFAULT
}

fn crawl_depth_default() -> u32 {
    CRAWL_DEPTH_MAX
}

/// Lower case, accents dropped (relevance reads "Éligible" as "eligible").
pub fn fold(text: &str) -> String {
    text.chars()
        .flat_map(char::to_lowercase)
        .map(|c| match c {
            'à' | 'â' | 'ä' | 'á' | 'ã' => 'a',
            'é' | 'è' | 'ê' | 'ë' => 'e',
            'î' | 'ï' | 'í' => 'i',
            'ô' | 'ö' | 'ó' | 'õ' => 'o',
            'ù' | 'û' | 'ü' | 'ú' => 'u',
            'ç' => 'c',
            'ÿ' => 'y',
            '\u{2019}' => '\'',
            c => c,
        })
        .collect()
}

/// Target and registry key of a declared site (D307): `web:site:<host>`,
/// the host normalised like the navigation rules (`site_host`), so a crawl
/// and the rules of one site share it whatever the scheme or `www.`.
pub fn site_key(origin: &str) -> String {
    let host = crate::site_recipe::site_host(origin).unwrap_or_else(|| origin.to_string());
    format!("web:site:{host}")
}

/// `https://host[:port]` of a site address.
pub fn site_origin(raw: &str) -> Option<String> {
    let u = url::Url::parse(raw.trim()).ok()?;
    let host = u.host_str()?.to_ascii_lowercase();
    if !matches!(u.scheme(), "http" | "https") || !u.username().is_empty() || u.password().is_some()
    {
        return None;
    }
    Some(match u.port() {
        Some(p) => format!("{}://{host}:{p}", u.scheme()),
        None => format!("{}://{host}", u.scheme()),
    })
}

/// `watch_live` (D260): record a live page while it is on air, in parts
/// of at most [`LIVE_PART_MAX_MS`], each a `video` media.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct WatchLiveParams {
    /// http(s) page of the live.
    pub url: String,
}

/// Message the install key signs to upload a part of a recorded live.
pub fn live_upload_message(
    device_id: &str,
    ts_ms: i64,
    mission_id: &str,
    part: u32,
    body_sha256: &str,
) -> String {
    format!("pnex-ext-live\n{device_id}\n{ts_ms}\n{mission_id}\n{part}\n{body_sha256}")
}

/// Answer of a live part upload.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct LivePartStored {
    #[cfg_attr(feature = "openapi", schema(format = "uuid"))]
    pub asset_id: String,
    pub part: u32,
}

/// Longest part of a recorded live (keeps a part under [`VIDEO_MAX`]).
pub const LIVE_PART_MAX_MS: i64 = 30 * 60 * 1000;

impl MissionSpec {
    pub fn kind(&self) -> &'static str {
        match self {
            Self::CollectValue(_) => "collect_value",
            Self::CollectPage(_) => "collect_page",
            Self::FollowAccount(_) => "follow_account",
            Self::WatchLive(_) => "watch_live",
            Self::CrawlSite(_) => "crawl_site",
            Self::CaptureProof(_) => "capture_proof",
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
    err_codes::EXT_DRM_PROTECTED,
    err_codes::EXT_REGISTRY_FULL,
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
        MissionSpec::WatchLive(p) => {
            let (url, page_target) = check_page_url(&p.url, "spec.params.url")?;
            // A live occupies its platform's tab (D255); one lease per live.
            let host = page_target
                .trim_start_matches("web:")
                .split('/')
                .next()
                .unwrap_or_default();
            let target = match host {
                "www.youtube.com" | "youtube.com" | "m.youtube.com" => {
                    format!("youtube:live:{url}")
                }
                "x.com" | "twitter.com" => format!("x:live:{url}"),
                "www.tiktok.com" | "tiktok.com" => format!("tiktok:live:{url}"),
                _ => page_target,
            };
            Ok((MissionSpec::WatchLive(WatchLiveParams { url }), target))
        }
        // Proofs are asked by the server (D308), never created by hand.
        MissionSpec::CaptureProof(_) => Err(field("spec.kind", err_codes::FIELD_INVALID)),
        MissionSpec::CrawlSite(p) => {
            let f = |k: &str| format!("spec.params.{k}");
            let origin =
                site_origin(&p.url).ok_or_else(|| field(&f("url"), err_codes::FIELD_INVALID))?;
            if p.max_pages == 0 || p.max_pages > CRAWL_PAGES_MAX {
                return Err(field(&f("max_pages"), err_codes::FIELD_INVALID));
            }
            if p.max_depth == 0 || p.max_depth > CRAWL_DEPTH_MAX {
                return Err(field(&f("max_depth"), err_codes::FIELD_INVALID));
            }
            if p.exclude.len() > 20
                || p.exclude
                    .iter()
                    .any(|x| !x.starts_with('/') || x.len() > 255)
            {
                return Err(field(&f("exclude"), err_codes::FIELD_INVALID));
            }
            if p.matchers.len() > MATCHERS_MAX
                || p.matchers
                    .iter()
                    .any(|m| m.value.trim().is_empty() || m.value.chars().count() > MATCH_VALUE_MAX)
            {
                return Err(field(&f("match"), err_codes::FIELD_INVALID));
            }
            if let Some(r) = &p.record {
                let known = |k: &str| RECORD_FIELDS.contains(&k);
                if !crate::ontology::schema::valid_key(&r.type_key)
                    || !r.properties.contains_key("url")
                    || r.properties.keys().any(|k| !known(k))
                    || r.properties
                        .values()
                        .any(|v| !crate::ontology::schema::valid_key(v))
                    || r.link_type.is_some() != r.link_to.is_some()
                    || r.proof_link
                        .as_deref()
                        .is_some_and(|l| !crate::ontology::schema::valid_key(l))
                    || r.link_to
                        .as_deref()
                        .is_some_and(|i| uuid::Uuid::parse_str(i).is_err())
                {
                    return Err(field(&f("record"), err_codes::FIELD_INVALID));
                }
            }
            let mut out = p.clone();
            out.url = origin.clone();
            out.exclude = p.exclude.iter().map(|x| x.trim().to_string()).collect();
            Ok((MissionSpec::CrawlSite(out), site_key(&origin)))
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
            let publisher_id = match p.publisher_id.as_deref().map(str::trim) {
                None | Some("") => None,
                Some(id) => Some(
                    uuid::Uuid::parse_str(id)
                        .map_err(|_| {
                            field("spec.params.publisher_id", err_codes::FIELD_INVALID_UUID)
                        })?
                        .to_string(),
                ),
            };
            Ok((
                MissionSpec::FollowAccount(FollowAccountParams {
                    platform: p.platform,
                    handle: handle.into(),
                    force_video: p.force_video,
                    publisher_id,
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
pub fn check_page_url(raw: &str, f: &str) -> Result<(String, String), FieldError> {
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
    /// `follow_account`: scrolls to do on each list page this visit (D283):
    /// growing while the account's history is being collected, then
    /// [`RECENT_DEPTH`].
    #[serde(default)]
    pub scroll_depth: u32,
    /// The agent brings the mission's tab to the front while reading it
    /// (D269): platforms that pause hidden, and any platform on an agent
    /// dedicated to it (a hidden tab loads no more of a list).
    #[serde(default)]
    pub foreground: bool,
}

/// Scrolls of a visit once the history is collected (recent posts only).
pub const RECENT_DEPTH: u32 = 3;
/// How much deeper each visit goes while collecting the history.
pub const HISTORY_STEP: u32 = 20;
/// Deepest visit (a few hundred scrolls cover any account's history).
pub const HISTORY_MAX_DEPTH: u32 = 400;

/// Depth of the next visit from the progress kept by the server.
pub fn next_depth(history_depth: u32, history_done: bool) -> u32 {
    if history_done {
        RECENT_DEPTH
    } else {
        (history_depth + HISTORY_STEP).min(HISTORY_MAX_DEPTH)
    }
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
        /// New videos whose platform captions the agent should read (D284:
        /// text by default, with or without the file).
        #[serde(default)]
        captions: Vec<String>,
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
        /// Scrolls actually done this visit.
        #[serde(default)]
        depth: u32,
        /// The list stopped loading: the start of the account is reached.
        #[serde(default)]
        end_reached: bool,
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
    /// When the post is a video: its file (best progressive mp4) when the
    /// platform exposes one, else its watch page (recorded in real time).
    #[serde(default)]
    pub video_url: Option<String>,
    #[serde(default)]
    pub duration_ms: Option<i64>,
}

/// Message the install key signs to upload a video (D262): bound to the
/// mission, the post, the part (1-based, O92) and the SHA-256 (hex) of
/// the file.
pub fn video_upload_message(
    device_id: &str,
    ts_ms: i64,
    mission_id: &str,
    platform_id: &str,
    part: u32,
    body_sha256: &str,
) -> String {
    format!(
        "pnex-ext-video\n{device_id}\n{ts_ms}\n{mission_id}\n{platform_id}\n{part}\n{body_sha256}"
    )
}

/// Message the install key signs to upload a post's captions (D281).
pub fn captions_upload_message(
    device_id: &str,
    ts_ms: i64,
    mission_id: &str,
    platform_id: &str,
    body_sha256: &str,
) -> String {
    format!("pnex-ext-captions\n{device_id}\n{ts_ms}\n{mission_id}\n{platform_id}\n{body_sha256}")
}

/// One caption of a post, on the video's clock.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct CaptionCue {
    pub start_ms: i64,
    pub end_ms: i64,
    pub text: String,
}

/// Captions of a post read on the platform (stored as a run of origin
/// `platform`, D281).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct CaptionsUpload {
    #[cfg_attr(feature = "openapi", schema(format = "uuid"))]
    pub mission_id: String,
    pub platform_id: String,
    /// BCP 47 language of the track (`fr`, `en-US`).
    pub lang: String,
    pub cues: Vec<CaptionCue>,
}

/// Most browser agents an org may pair (extension-collector.md §13).
pub const AGENTS_MAX: i64 = 20;
/// Most missions an org may hold (extension-collector.md §13).
pub const MISSIONS_MAX: i64 = 200;
/// Most parts of one post's video (16 × 30 min, O92).
pub const VIDEO_PARTS_MAX: u32 = 16;
/// Most posts an org's video registry keeps (observations.md O89).
pub const VIDEOS_MAX: i64 = 500_000;

/// Largest captured video (bytes).
pub const VIDEO_MAX: usize = 200 * 1024 * 1024;

/// State of a video in the registry (D259).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "snake_case")]
pub enum VideoState {
    /// Known, not downloaded (metadata only, or capture pending/failed).
    Seen,
    /// Its platform captions are in the media library, not its file.
    Captioned,
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

    #[test]
    fn a_site_key_ignores_scheme_and_www() {
        assert_eq!(site_key("https://www.LaStrat.fr"), "web:site:lastrat.fr");
        assert_eq!(
            site_key("http://lastrat.fr"),
            site_key("https://lastrat.fr")
        );
    }

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
            force_video: true,
            publisher_id: None,
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
            force_video: false,
            publisher_id: None,
        });
        assert_eq!(check_spec(&bad).unwrap_err().0, "spec.params.handle");
        let json =
            serde_json::json!({"kind":"follow_account","params":{"platform":"x","handle":"a"}});
        let parsed: MissionSpec = serde_json::from_value(json).unwrap();
        assert!(matches!(
            parsed,
            MissionSpec::FollowAccount(FollowAccountParams {
                force_video: false,
                ..
            })
        ));
    }

    #[test]
    fn modes_split_exclusive_platforms() {
        assert!(mode_allows("shared", "x"));
        assert!(mode_allows("shared", "web:example.org"));
        assert!(
            !mode_allows("shared", "tiktok"),
            "TikTok needs a dedicated agent"
        );
        assert!(mode_allows("dedicated:tiktok", "tiktok"));
        assert!(!mode_allows("dedicated:tiktok", "x"));
        // A hidden tab loads no more of a list: a dedicated agent reads in front.
        assert!(!runs_in_front("shared", "youtube"));
        assert!(runs_in_front("shared", "tiktok"));
        assert!(runs_in_front("dedicated:youtube", "youtube"));
        assert!(check_mode("dedicated:youtube").is_ok());
        assert!(check_mode("dedicated:web").is_err());
        let spec = MissionSpec::FollowAccount(FollowAccountParams {
            platform: SocialPlatform::X,
            handle: "a".into(),
            force_video: false,
            publisher_id: Some("not-a-uuid".into()),
        });
        assert_eq!(check_spec(&spec).unwrap_err().0, "spec.params.publisher_id");
    }

    #[test]
    fn a_live_takes_its_platform_tab() {
        let live = |url: &str| {
            let spec = MissionSpec::WatchLive(WatchLiveParams { url: url.into() });
            check_spec(&spec).unwrap().1
        };
        assert_eq!(
            platform_of(&live("https://www.youtube.com/watch?v=abc")),
            "youtube"
        );
        assert_eq!(platform_of(&live("https://x.com/i/broadcasts/1")), "x");
        assert_eq!(
            platform_of(&live("https://www.tiktok.com/@a/live")),
            "tiktok"
        );
        assert_eq!(
            platform_of(&live("https://tv.example.org/direct")),
            "web:tv.example.org"
        );
        assert!(SocialPlatform::TikTok.needs_foreground());
        assert_eq!(
            SocialPlatform::parse("tiktok"),
            Some(SocialPlatform::TikTok)
        );
    }

    #[test]
    fn history_depth_grows_then_stays_recent() {
        assert_eq!(next_depth(0, false), HISTORY_STEP);
        assert_eq!(next_depth(HISTORY_STEP, false), 2 * HISTORY_STEP);
        assert_eq!(next_depth(HISTORY_MAX_DEPTH, false), HISTORY_MAX_DEPTH);
        assert_eq!(next_depth(120, true), RECENT_DEPTH);
        assert_eq!(SocialPlatform::YouTube.list_urls("chan").len(), 2);
        assert_eq!(
            SocialPlatform::parse("youtube"),
            Some(SocialPlatform::YouTube)
        );
        let yt: SocialPlatform = serde_json::from_value(serde_json::json!("youtube")).unwrap();
        assert_eq!(yt, SocialPlatform::YouTube);
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
