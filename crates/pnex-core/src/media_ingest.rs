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
    /// IP camera over RTSP (TCP interleaved only, D160).
    Rtsp,
}

impl MediaStreamKind {
    pub const ALL: [MediaStreamKind; 4] = [Self::Icecast, Self::Hls, Self::HttpFile, Self::Rtsp];

    pub fn wire(self) -> &'static str {
        match self {
            Self::Icecast => "icecast",
            Self::Hls => "hls",
            Self::HttpFile => "http_file",
            Self::Rtsp => "rtsp",
        }
    }

    /// URL scheme(s) a stream of this kind accepts.
    pub fn scheme_ok(self, scheme: &str) -> bool {
        match self {
            Self::Rtsp => scheme == "rtsp",
            _ => matches!(scheme, "http" | "https"),
        }
    }

    pub fn from_wire(s: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|k| k.wire() == s)
    }
}

/// Tracks captured from a stream (D175): audio goes to the ASR path,
/// video frames to the camera bus.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum MediaTracks {
    #[default]
    #[serde(rename = "audio")]
    Audio,
    #[serde(rename = "video")]
    Video,
    #[serde(rename = "audio+video")]
    AudioVideo,
}

impl MediaTracks {
    pub const ALL: [MediaTracks; 3] = [Self::Audio, Self::Video, Self::AudioVideo];

    pub fn wire(self) -> &'static str {
        match self {
            Self::Audio => "audio",
            Self::Video => "video",
            Self::AudioVideo => "audio+video",
        }
    }

    pub fn from_wire(s: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|t| t.wire() == s)
    }

    pub fn has_audio(self) -> bool {
        self != Self::Video
    }

    pub fn has_video(self) -> bool {
        self != Self::Audio
    }
}

/// Bounds of the video sampling rate of a stream (D159, D175).
pub const FPS_MIN: i32 = 1;
pub const FPS_MAX: i32 = 5;
pub const FPS_DEFAULT: i32 = 1;
/// Widest frame published on the bus (larger frames are scaled down).
pub const FRAME_MAX_WIDTH: u32 = 1280;

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

    pub fn from_wire(s: &str) -> Option<Self> {
        [Self::Pdt, Self::Tdt, Self::Host]
            .into_iter()
            .find(|k| k.wire() == s)
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

/// Valkey channel of the video frames of a stream (D175), in the camera
/// bus format. Own key space: never collides with a device camera
/// (`pnex:cam:v1:{org}:{device}`). Built from the stamped org and a
/// validated slug, never from a config string (R3).
pub fn frame_channel(org_id: i64, slug: &str) -> String {
    format!("pnex:media:v1:{org_id}:{slug}:cam")
}

/// Valkey key of one frame of a stream (TTL `camera::FRAME_TTL_SECS`).
pub fn frame_key(org_id: i64, slug: &str, seq: u32) -> String {
    format!("pnex:media:v1:{org_id}:{slug}:cam:f:{seq}")
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
    #[serde(default)]
    pub tracks: MediaTracks,
    /// Video frames per second (D175), used when `tracks` has video.
    #[serde(default = "default_fps")]
    pub fps: i32,
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
    /// Capture health, enabled streams only (D160).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub health: Option<StreamHealth>,
    /// Name (`device_id`) of the capture box when `capture_on` is
    /// `device:<id>` (lot 6b).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub capture_device: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

fn default_fps() -> i32 {
    FPS_DEFAULT
}

/// Capture health of an enabled stream, computed when read (D160, §9).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StreamHealth {
    /// Seconds since the last captured audio.
    pub gap_secs: i64,
    /// Age of the oldest segment waiting for its transcription.
    pub lag_secs: i64,
    /// Transcribed share of the last hour, 0..=1.
    pub coverage: f64,
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
    pub tracks: Option<MediaTracks>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fps: Option<i32>,
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
    /// Diarization = a segmentation model (`diarization_model_id`) and a
    /// speaker embedding model, both set or both absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub diarization_model_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub diarization_embedding_model_id: Option<String>,
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
    /// Segmentation model of the diarization; empty string clears it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub diarization_model_id: Option<String>,
    /// Speaker embedding model of the diarization; empty string clears it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub diarization_embedding_model_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub language: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub beam: Option<i32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub word_timestamps: Option<bool>,
}

/// Check of an audio model on the server (D167): load + transcription of
/// the reference sample.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct AsrModelCheck {
    /// `unchecked` | `valid` | `invalid` (same values as vision).
    pub status: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub load_ms: Option<u64>,
    /// Inference time / audio time.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rtf: Option<f64>,
    /// Word error rate on the French reference sample.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wer: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub checked_at: Option<String>,
}

impl AsrModelCheck {
    /// Real-time streams one process sustains (`≈ 1 / rtf`).
    pub fn streams(&self) -> Option<f64> {
        self.rtf.filter(|r| *r > 0.0).map(|r| 1.0 / r)
    }
}

/// Check of a model on one carrier (`worker:<host>` or a name set by the
/// platform).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AsrCarrierCheck {
    pub carrier: String,
    pub check: AsrModelCheck,
}

/// Audio model of the registry (D167): a `model` media asset (archive or
/// GGML file) read by the `pnex-asr` runtime.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AsrModel {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub description: String,
    /// `asr` | `vad` | `diarization_segmentation` | `speaker_embedding`,
    /// read from the files like the family.
    #[serde(default)]
    pub task: String,
    /// Read from the files (`parakeet_tdt`, `canary`, `whisper`,
    /// `whisper_ggml`, `silero`, `pyannote_segmentation`,
    /// `speaker_embedding`), never typed.
    pub family: String,
    pub asset_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub asset_version: Option<i64>,
    /// SPDX identifier stated at import.
    pub license: String,
    /// Check on the server (where `ml_models.check_status` comes from).
    pub check: AsrModelCheck,
    /// Checks on the dedicated transcription workers (D167), one per
    /// carrier.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub carriers: Vec<AsrCarrierCheck>,
    pub created_at: String,
    pub updated_at: String,
}

/// Create / update body of an audio model; absent fields keep their value.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct AsrModelInput {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub asset_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub asset_version: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub license: Option<String>,
}

/// Result of `POST /api/v1/media/streams/{id}/test` (§7): a first extract
/// captured, then transcribed when the stream has a usable profile.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct StreamTestResult {
    /// Capture failure code (same codes as `capture_error`); `None` = an
    /// extract was captured.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub capture_error: Option<String>,
    pub audio_ms: u64,
    /// Text of the extract; `None` when it was not transcribed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    /// Why the extract was not transcribed: `no-profile`, `model-invalid`,
    /// `asr-failed`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub transcribe_error: Option<String>,
}

/// Longest clip transcribed by the model test (D167), seconds.
pub const TEST_CLIP_MAX_SECS: u32 = 120;

/// One timed word of a test transcription.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AsrTestWord {
    pub word: String,
    pub start_ms: u32,
    pub end_ms: u32,
}

/// Result of `POST /api/v1/asr/models/{id}/test`: an uploaded clip
/// transcribed by the model (first [`TEST_CLIP_MAX_SECS`] only).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AsrTestResult {
    pub text: String,
    #[serde(default)]
    pub words: Vec<AsrTestWord>,
    /// Audio actually transcribed.
    pub audio_ms: u64,
    pub infer_ms: u64,
    /// True when the clip was longer and was cut.
    #[serde(default)]
    pub truncated: bool,
}

/// Licenses offered at import (SPDX). Non-commercial ones are flagged.
pub const ASR_LICENSES: [&str; 6] = [
    "Apache-2.0",
    "MIT",
    "CC-BY-4.0",
    "CC-BY-SA-4.0",
    "CC-BY-NC-4.0",
    "other",
];

/// A license that forbids commercial use.
pub fn license_is_non_commercial(spdx: &str) -> bool {
    spdx.contains("-NC")
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
    /// Speaker turns as a JSON string, `[{label, start_ms, end_ms}]`;
    /// labels are local to the stream (`S1`…), never identities.
    #[serde(default)]
    pub speakers: String,
    #[serde(default)]
    pub lang: String,
    #[serde(default)]
    pub asr_model: String,
}

/// One in-band metadata event as stored in `mx_<slug>` (D170) and
/// returned by `GET /api/v1/media/metadata`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MetadataRecord {
    /// Reception time, RFC 3339.
    pub ts: String,
    pub stream: String,
    /// `icy_title` for now.
    pub kind: String,
    pub text: String,
}

/// `kind` of an ICY `StreamTitle` change.
pub const METADATA_KIND_ICY_TITLE: &str = "icy_title";

/// Emission granularity of the `media_source` flow node (D163).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MediaSourceEmit {
    /// One message per transcribed segment.
    #[default]
    Segment,
    /// One message per sentence of each segment (split by the node).
    Sentence,
}

/// Configuration of the `media_source` flow node (D163): event source of
/// the transcribed segments of one or more streams of the org.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct MediaSourceConfig {
    /// Stream slugs. Existence in the org is checked at deploy.
    #[serde(default)]
    pub streams: Vec<String>,
    #[serde(default)]
    pub emit: MediaSourceEmit,
    /// Drop segments below this confidence (0..=1). Only applies when the
    /// segment carries a confidence: a segment without one passes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min_confidence: Option<f64>,
}

impl MediaSourceConfig {
    /// Structural check shared by the save validation and the runtime build.
    pub fn check(&self) -> Option<(&'static str, String)> {
        if self.streams.is_empty() {
            return Some((
                "media_source_no_stream",
                "select at least one stream".into(),
            ));
        }
        let mut seen = std::collections::BTreeSet::new();
        for slug in &self.streams {
            if !is_valid_slug(slug) {
                return Some((
                    "media_source_stream_invalid",
                    format!("invalid stream slug `{slug}`"),
                ));
            }
            if !seen.insert(slug) {
                return Some((
                    "media_source_stream_duplicate",
                    format!("stream `{slug}` is listed twice"),
                ));
            }
        }
        if let Some(c) = self.min_confidence {
            if !(0.0..=1.0).contains(&c) {
                return Some((
                    "media_source_confidence_invalid",
                    "min confidence must be between 0 and 1".into(),
                ));
            }
        }
        None
    }
}

// ───────────── Capture boxes (`capture_on = device:<id>`, lot 6b) ─────────────

/// Capability id an edge agent announces when it can capture streams
/// (D159: `Announce.caps`, family `feature`).
pub const MEDIA_CAPTURE_CAP: &str = "media_capture";

/// `capture_error` written by the server when a box drops its media link.
pub const CAPTURE_ERROR_DEVICE_OFFLINE: &str = "device-offline";

/// A capture box of the org (stream form picker).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MediaCaptureDevice {
    /// `device_registries.id` (`capture_on = device:<id>`).
    pub id: i64,
    /// Device name (`device_id`).
    pub name: String,
}

/// One stream a box captures, pushed over `/ws/media` (D160). `auth_secret`
/// is the stream's own access credential, sent to that box only over its
/// authenticated link (media-ingest.md §21, SEC-27); never a storage secret.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DeviceStream {
    pub id: String,
    pub slug: String,
    pub kind: MediaStreamKind,
    pub url: String,
    pub segment_secs: i32,
    pub overlap_secs: i32,
    #[serde(default)]
    pub tracks: MediaTracks,
    #[serde(default = "default_fps")]
    pub fps: i32,
    pub enabled: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auth_secret: Option<String>,
}

/// Server → box messages of `/ws/media` (sealed text frames).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "t", rename_all = "snake_case")]
pub enum MediaDownMsg {
    /// Full list of the box's streams (on connect and on every change).
    Streams { streams: Vec<DeviceStream> },
    /// Outcome of one uploaded segment; `code` is a short refusal reason.
    Ack {
        stream_id: String,
        seq: i64,
        ok: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        code: Option<String>,
    },
}

/// Box → server messages of `/ws/media` (sealed text frames).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "t", rename_all = "snake_case")]
pub enum MediaUpMsg {
    /// Capture state of one stream; `error` is a capture error code.
    State {
        stream_id: String,
        state: CaptureState,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        error: Option<String>,
    },
}

/// Header of an uploaded segment. Binary frame (sealed):
/// `u16 BE header length ‖ header JSON ‖ WAV bytes`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SegmentHeader {
    pub stream_id: String,
    pub seq: i64,
    pub started_ms: i64,
    pub ended_ms: i64,
    pub clock: ClockSource,
}

/// Builds the binary frame of a segment upload.
pub fn encode_segment(header: &SegmentHeader, wav: &[u8]) -> Vec<u8> {
    let head = serde_json::to_vec(header).unwrap_or_default();
    let mut out = Vec::with_capacity(2 + head.len() + wav.len());
    out.extend_from_slice(&(head.len() as u16).to_be_bytes());
    out.extend_from_slice(&head);
    out.extend_from_slice(wav);
    out
}

/// Splits a segment upload frame; `None` when malformed.
pub fn decode_segment(frame: &[u8]) -> Option<(SegmentHeader, &[u8])> {
    let len = u16::from_be_bytes([*frame.first()?, *frame.get(1)?]) as usize;
    let head = frame.get(2..2 + len)?;
    let header = serde_json::from_slice(head).ok()?;
    Some((header, &frame[2 + len..]))
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
    fn tracks_and_frame_keys() {
        for t in MediaTracks::ALL {
            assert_eq!(MediaTracks::from_wire(t.wire()), Some(t));
            assert_eq!(
                serde_json::to_string(&t).unwrap(),
                format!("\"{}\"", t.wire())
            );
        }
        assert!(MediaTracks::AudioVideo.has_audio() && MediaTracks::AudioVideo.has_video());
        assert!(!MediaTracks::Video.has_audio() && !MediaTracks::Audio.has_video());
        assert_eq!(frame_channel(7, "cam_1"), "pnex:media:v1:7:cam_1:cam");
        assert_eq!(frame_key(7, "cam_1", 3), "pnex:media:v1:7:cam_1:cam:f:3");
        // Never in the device camera key space.
        assert!(!frame_key(7, "cam_1", 3).starts_with("pnex:cam:"));
        assert!(MediaStreamKind::Rtsp.scheme_ok("rtsp"));
        assert!(!MediaStreamKind::Rtsp.scheme_ok("http"));
        assert!(!MediaStreamKind::Hls.scheme_ok("rtsp"));
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

    #[test]
    fn media_source_check() {
        let cfg = |streams: &[&str], c: Option<f64>| MediaSourceConfig {
            streams: streams.iter().map(|s| s.to_string()).collect(),
            emit: MediaSourceEmit::Segment,
            min_confidence: c,
        };
        let code = |c: &MediaSourceConfig| c.check().map(|(code, _)| code);
        assert_eq!(code(&cfg(&["inter", "rfi"], Some(0.5))), None);
        assert_eq!(code(&cfg(&[], None)), Some("media_source_no_stream"));
        assert_eq!(
            code(&cfg(&["A b"], None)),
            Some("media_source_stream_invalid")
        );
        assert_eq!(
            code(&cfg(&["x", "x"], None)),
            Some("media_source_stream_duplicate")
        );
        assert_eq!(
            code(&cfg(&["x"], Some(1.5))),
            Some("media_source_confidence_invalid")
        );
        assert_eq!(
            code(&cfg(&["x"], Some(f64::NAN))),
            Some("media_source_confidence_invalid")
        );
    }

    #[test]
    fn segment_frame_round_trip() {
        let h = SegmentHeader {
            stream_id: "s".into(),
            seq: 7,
            started_ms: 1,
            ended_ms: 2,
            clock: ClockSource::Host,
        };
        let frame = encode_segment(&h, b"RIFFdata");
        let (back, wav) = decode_segment(&frame).unwrap();
        assert_eq!(back, h);
        assert_eq!(wav, b"RIFFdata");
        assert!(decode_segment(&frame[..5]).is_none());
        assert!(decode_segment(&[0, 2, b'{', b'}']).is_none());
        let msg: MediaDownMsg =
            serde_json::from_str(r#"{"t":"ack","stream_id":"s","seq":1,"ok":true}"#).unwrap();
        assert!(matches!(msg, MediaDownMsg::Ack { ok: true, .. }));
    }
}
