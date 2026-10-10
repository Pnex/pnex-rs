//! Media ingest (media-ingest.md P2.13): declared streams captured into
//! audio segments, transcribed by the queue, text stored in O2.
//!
//! - [`capture`]: supervisor of `server` / `worker` captures (fetcher,
//!   confined ffmpeg, segmenter), [`segments`]: segment blobs and rows;
//! - [`health`]: capture metrics and the "silent stream" alert;
//! - [`streams`] / [`profiles`]: CRUD services shared by the HTTP
//!   controller (and later the assistant tools, D142 §9.3);
//! - [`MediaIngestSettings`]: platform settings (`settings.media_ingest`,
//!   `PNEX_MEDIA_*` overrides); [`asr_queue_tags`]: queue routing (D166).

pub mod asr;
pub mod capture;
pub mod health;
pub mod models;
pub mod profiles;
pub mod segments;
pub mod streams;
pub mod transcripts;

use loco_rs::config::Config;
use serde::Deserialize;

/// Platform settings of media ingest.
#[derive(Debug, Clone)]
pub struct MediaIngestSettings {
    /// Max declared streams per org (D159 quota).
    pub max_streams_per_org: u64,
}

impl Default for MediaIngestSettings {
    fn default() -> Self {
        Self {
            max_streams_per_org: 3,
        }
    }
}

#[derive(Debug, Default, Deserialize)]
struct Partial {
    max_streams_per_org: Option<u64>,
}

impl MediaIngestSettings {
    pub fn from_config(config: &Config) -> Self {
        let partial: Partial = config
            .settings
            .as_ref()
            .and_then(|s| s.get("media_ingest"))
            .and_then(|v| serde_json::from_value(v.clone()).ok())
            .unwrap_or_default();
        let mut out = Self::default();
        if let Some(v) = partial.max_streams_per_org {
            out.max_streams_per_org = v;
        }
        if let Some(v) = env_parse::<u64>("PNEX_MEDIA_MAX_STREAMS_PER_ORG") {
            out.max_streams_per_org = v;
        }
        out
    }
}

fn env_parse<T: std::str::FromStr>(key: &str) -> Option<T> {
    std::env::var(key).ok().and_then(|v| v.trim().parse().ok())
}

/// Loco tags of the transcription jobs (D166): `PNEX_ASR_QUEUE_TAG`, read
/// identically by the enqueuer and every worker process; empty = untagged.
pub fn asr_queue_tags() -> Vec<String> {
    std::env::var("PNEX_ASR_QUEUE_TAG")
        .ok()
        .map(|t| t.trim().to_string())
        .filter(|t| !t.is_empty())
        .into_iter()
        .collect()
}
