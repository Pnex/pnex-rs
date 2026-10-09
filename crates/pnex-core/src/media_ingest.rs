//! Media ingest contract (media-ingest.md P2.13, D159–D166) — streams
//! captured into transcribed audio segments. Pure, wasm-safe.
//!
//! - [`MediaStreamKind`], [`CaptureOn`], [`AudioRetention`]: stream config;
//! - [`SegmentState`], [`ClockSource`], [`CaptureState`]: runtime states;
//! - [`stream_slug`], [`transcript_stream`], [`transcript_channel`]: names
//!   built server side from the slug (R3, R18), never from client input;
//! - [`url_has_credentials`]: stream URLs never carry a secret (D159);
//! - DTOs: [`MediaStream`], [`MediaSegment`], [`AsrProfile`],
//!   [`TranscriptRecord`].

use serde::{Deserialize, Serialize};

/// Max length of a stream slug (same bound as event streams, D84).
pub const SLUG_MAX_LEN: usize = 48;
/// Bounds of `segment_secs` (D159).
pub const SEGMENT_SECS_MIN: i32 = 10;
pub const SEGMENT_SECS_MAX: i32 = 120;
pub const SEGMENT_SECS_DEFAULT: i32 = 30;
/// Bounds of `overlap_secs` (D159).
pub const OVERLAP_SECS_MAX: i32 = 3;
pub const OVERLAP_SECS_DEFAULT: i32 = 1;
/// Upper bound of a sliding audio retention, in days (D161).
pub const RETENTION_DAYS_MAX: u16 = 365;
/// Max length of a stream URL.
pub const URL_MAX_LEN: usize = 2048;
/// Sample rate of captured PCM segments (16 kHz mono f32, `pnex-asr`).
pub const SAMPLE_RATE: u32 = 16_000;

/// Transport of a stream. Lot 1 captures the HTTP family; the other kinds
/// of D159 (DASH, RTSP, DVB) join with their fetcher.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MediaStreamKind {
    /// Icecast / Shoutcast continuous HTTP audio.
    #[default]
    Icecast,
    /// HTTP Live Streaming playlist.
    Hls,
    /// A finite audio file over HTTP (podcast episode, replay).
    HttpFile,
}

impl MediaStreamKind {
    pub const ALL: [MediaStreamKind; 3] = [Self::Icecast, Self::Hls, Self::HttpFile];

    pub fn wire(self) -> &'static str {
        match self {
            Self::Icecast => "icecast",
            Self::Hls => "hls",
            Self::HttpFile => "http_file",
        }
    }

    pub fn from_wire(s: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|k| k.wire() == s)
    }
}

/// Where the capture runs (D160).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CaptureOn {
    /// Supervisor inside the server binary (all-in-one, Pi).
    #[default]
    Server,
    /// A queue worker of the mesh (`/internal/media/segment`).
    Worker,
    /// An edge agent (`device_registries.id`), lot 6.
    Device(i64),
}

impl CaptureOn {
    pub fn wire(self) -> String {
        match self {
            Self::Server => "server".into(),
            Self::Worker => "worker".into(),
            Self::Device(id) => format!("device:{id}"),
        }
    }

    pub fn from_wire(s: &str) -> Option<Self> {
        match s {
            "server" => Some(Self::Server),
            "worker" => Some(Self::Worker),
            _ => s
                .strip_prefix("device:")
                .and_then(|id| id.parse::<i64>().ok())
                .filter(|id| *id > 0)
                .map(Self::Device),
        }
    }
}

/// How long captured audio is kept (D161). Transcribed text is the only
/// durable output.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum AudioRetention {
    /// Blob deleted as soon as the transcription succeeds.
    #[default]
    None,
    /// Sliding retention, pruned hourly.
    Days(u16),
    /// Kept until the stream is deleted (rights held by the org).
    Keep,
}

impl AudioRetention {
    pub fn wire(self) -> String {
        match self {
            Self::None => "none".into(),
            Self::Days(n) => format!("days:{n}"),
            Self::Keep => "keep".into(),
        }
    }

    pub fn from_wire(s: &str) -> Option<Self> {
        match s {
            "none" => Some(Self::None),
            "keep" => Some(Self::Keep),
            _ => s
                .strip_prefix("days:")
                .and_then(|n| n.parse::<u16>().ok())
                .filter(|n| (1..=RETENTION_DAYS_MAX).contains(n))
                .map(Self::Days),
        }
    }
}

/// Lifecycle of a segment (D162). Gaps are data: `failed` and `skipped_*`
/// rows stay visible in the coverage.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SegmentState {
    Captured,
    Queued,
    Transcribing,
    Transcribed,
    Failed,
    SkippedSilence,
    SkippedBacklog,
}

impl SegmentState {
    pub const ALL: [SegmentState; 7] = [
        Self::Captured,
        Self::Queued,
        Self::Transcribing,
        Self::Transcribed,
        Self::Failed,
        Self::SkippedSilence,
        Self::SkippedBacklog,
    ];

    pub fn wire(self) -> &'static str {
        match self {
            Self::Captured => "captured",
            Self::Queued => "queued",
            Self::Transcribing => "transcribing",
            Self::Transcribed => "transcribed",
            Self::Failed => "failed",
            Self::SkippedSilence => "skipped_silence",
            Self::SkippedBacklog => "skipped_backlog",
        }
    }

    pub fn from_wire(s: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|k| k.wire() == s)
    }

    /// No worker touches the segment again.
    pub fn is_terminal(self) -> bool {
        matches!(
            self,
            Self::Transcribed | Self::Failed | Self::SkippedSilence | Self::SkippedBacklog
        )
    }
}

/// Where the absolute time of a segment comes from (D160): a statistic must
/// be able to say where its clock comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ClockSource {
    /// `EXT-X-PROGRAM-DATE-TIME` of the HLS playlist.
    Pdt,
    /// Time and date table of a DVB signal.
    Tdt,
    /// NTP clock of the carrier when the segment started.
    Host,
}

impl ClockSource {
    pub fn wire(self) -> &'static str {
        match self {
            Self::Pdt => "pdt",
            Self::Tdt => "tdt",
            Self::Host => "host",
        }
    }
}

/// Capture supervisor state of a stream, shown on /media.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CaptureState {
    #[default]
    Stopped,
    Starting,
    Running,
    /// Waiting before a restart (exponential backoff).
    Backoff,
}

impl CaptureState {
    pub fn wire(self) -> &'static str {
        match self {
            Self::Stopped => "stopped",
            Self::Starting => "starting",
            Self::Running => "running",
            Self::Backoff => "backoff",
        }
    }

    pub fn from_wire(s: &str) -> Option<Self> {
        [Self::Stopped, Self::Starting, Self::Running, Self::Backoff]
            .into_iter()
            .find(|k| k.wire() == s)
    }
}

/// Stream name → slug: lowercase, `[a-z0-9_]`, other characters folded to
/// `_`. `None` when nothing usable is left. Longer names are cut at
/// [`SLUG_MAX_LEN`]; uniqueness is the caller's job (suffix on conflict).
pub fn stream_slug(name: &str) -> Option<String> {
    let mut slug = String::with_capacity(name.len());
    for c in name.trim().chars() {
        let c = c.to_ascii_lowercase();
        if c.is_ascii_alphanumeric() {
            slug.push(c);
        } else if !slug.is_empty() && !slug.ends_with('_') {
            slug.push('_');
        }
    }
    slug.truncate(SLUG_MAX_LEN);
    let slug = slug.trim_matches('_');
    (!slug.is_empty()).then(|| slug.to_string())
}

/// True when `slug` is what [`stream_slug`] produces (re-check of stored or
/// received slugs before they name an O2 stream or a Valkey channel).
pub fn is_valid_slug(slug: &str) -> bool {
    !slug.is_empty()
        && slug.len() <= SLUG_MAX_LEN
        && !slug.starts_with('_')
        && !slug.ends_with('_')
        && !slug.contains("__")
        && slug
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
}

/// O2 logs stream of the transcriptions of a stream (D165).
pub fn transcript_stream(slug: &str) -> String {
    format!("tx_{slug}")
}

/// O2 logs stream of in-band metadata (D170).
pub fn metadata_stream(slug: &str) -> String {
    format!("mx_{slug}")
}

/// Valkey channel of transcribed segment events (D163). Built from the
/// stamped org and the slug, never from a config string (R3).
pub fn transcript_channel(org_id: i64, slug: &str) -> String {
    format!("pnex:media:v1:{org_id}:{slug}:tx")
}

/// Query parameter names that carry a credential.
const CREDENTIAL_PARAMS: [&str; 12] = [
    "token",
    "access_token",
    "auth",
    "key",
    "apikey",
    "api_key",
    "password",
    "passwd",
    "secret",
    "signature",
    "sig",
    "jwt",
];

/// A stream URL must not carry a credential (D159): userinfo, or a query
/// parameter whose name says it is one. The credential goes in the stream's
/// vault secret instead.
pub fn url_has_credentials(url: &url::Url) -> bool {
    if !url.username().is_empty() || url.password().is_some() {
        return true;
    }
    url.query_pairs().any(|(k, _)| {
        let k = k.to_ascii_lowercase();
        CREDENTIAL_PARAMS.contains(&k.as_str())
            || k.ends_with("_token")
            || k.ends_with("-token")
            || k.ends_with("_key")
    })
}

/// Read DTO of a stream. Never carries the secret value (R4, R16): only
/// whether one is set.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MediaStream {
    pub id: String,
    pub name: String,
    pub slug: String,
    pub kind: MediaStreamKind,
    pub url: String,
    pub has_secret: bool,
    /// Reference to the vault secret (name, never the value).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auth_secret: Option<crate::secrets::SecretFieldView>,
    pub enabled: bool,
    /// `server` | `worker` | `device:<id>`.
    pub capture_on: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub asr_profile_id: Option<String>,
    pub segment_secs: i32,
    pub overlap_secs: i32,
    /// `none` | `days:N` | `keep`.
    pub audio_retention: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notify_channel_id: Option<String>,
    pub timezone: String,
    /// Date of the text-and-data-mining opt-out check (D172); `None` =
    /// persistent warning.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tdm_checked_at: Option<String>,
    #[serde(default)]
    pub tdm_note: String,
    pub capture_state: CaptureState,
    /// Short machine code of the last capture failure.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub capture_error: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

/// Create / update body of a stream. On update every field is optional
/// and absent fields keep their value.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct MediaStreamInput {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<MediaStreamKind>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    /// Vault secret sent to the URL's origin only (D159, R9).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auth_secret: Option<crate::secrets::SecretFieldInput>,
    /// Removes the stream's secret.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub clear_secret: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enabled: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub capture_on: Option<String>,
    /// Empty string clears the profile.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub asr_profile_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub segment_secs: Option<i32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub overlap_secs: Option<i32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub audio_retention: Option<String>,
    /// Empty string clears the channel.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notify_channel_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timezone: Option<String>,
    /// `true` stamps the TDM check now, `false` clears it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tdm_checked: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tdm_note: Option<String>,
}

/// Read DTO of a segment (D162).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MediaSegment {
    pub id: String,
    pub seq: i64,
    pub started_at: String,
    pub ended_at: String,
    pub clock_source: String,
    pub state: SegmentState,
    pub size_bytes: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub asr_model_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub asr_ms: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// Transcription profile (D164).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AsrProfile {
    pub id: String,
    pub name: String,
    pub asr_model_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub vad_model_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub diarization_model_id: Option<String>,
    /// ISO 639-1 code, or `auto`.
    pub language: String,
    pub beam: i32,
    pub word_timestamps: bool,
    pub created_at: String,
    pub updated_at: String,
}

/// Create / update body of a profile; absent fields keep their value.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct AsrProfileInput {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub asr_model_id: Option<String>,
    /// Empty string clears the model.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub vad_model_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub language: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub beam: Option<i32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub word_timestamps: Option<bool>,
}

/// True for `auto` or a two/three-letter lowercase language code.
pub fn is_valid_language(s: &str) -> bool {
    s == "auto" || ((2..=3).contains(&s.len()) && s.bytes().all(|b| b.is_ascii_lowercase()))
}

/// One transcribed segment as stored in `tx_<slug>` (D165) and returned by
/// `GET /api/v1/media/transcripts`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TranscriptRecord {
    /// Segment start, RFC 3339.
    pub ts: String,
    pub stream: String,
    pub segment_id: String,
    pub text: String,
    /// Word timings as a JSON string (O2 flattens nested objects).
    #[serde(default)]
    pub words: String,
    #[serde(default)]
    pub lang: String,
    #[serde(default)]
    pub asr_model: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slug_folds_and_bounds() {
        assert_eq!(stream_slug("France Inter").as_deref(), Some("france_inter"));
        assert_eq!(
            stream_slug("  -- RFI / Afrique --").as_deref(),
            Some("rfi_afrique")
        );
        assert_eq!(stream_slug("€€€"), None);
        let long = "a".repeat(80);
        assert_eq!(stream_slug(&long).unwrap().len(), SLUG_MAX_LEN);
        assert!(is_valid_slug("france_inter_2"));
        for bad in ["", "_x", "x_", "a__b", "A", "a-b", "a:b"] {
            assert!(!is_valid_slug(bad), "{bad}");
        }
    }

    #[test]
    fn capture_on_and_retention_round_trip() {
        for s in ["server", "worker", "device:42"] {
            assert_eq!(CaptureOn::from_wire(s).unwrap().wire(), s);
        }
        for bad in ["device:", "device:-1", "device:0", "gpu"] {
            assert!(CaptureOn::from_wire(bad).is_none(), "{bad}");
        }
        for s in ["none", "keep", "days:7"] {
            assert_eq!(AudioRetention::from_wire(s).unwrap().wire(), s);
        }
        for bad in ["days:0", "days:366", "days:x", "forever"] {
            assert!(AudioRetention::from_wire(bad).is_none(), "{bad}");
        }
    }

    #[test]
    fn credentials_in_url_are_detected() {
        let has = |s: &str| url_has_credentials(&url::Url::parse(s).unwrap());
        assert!(has("https://user:pw@radio.example/live"));
        assert!(has("https://radio.example/live.m3u8?token=abc"));
        assert!(has("https://radio.example/live?Access_Token=abc"));
        assert!(has("https://radio.example/live?hdnts_key=abc"));
        assert!(!has("https://radio.example/live.mp3?id=franceinter"));
    }

    #[test]
    fn names_derive_from_slug() {
        assert_eq!(transcript_stream("inter"), "tx_inter");
        assert_eq!(metadata_stream("inter"), "mx_inter");
        assert_eq!(transcript_channel(7, "inter"), "pnex:media:v1:7:inter:tx");
    }
}
