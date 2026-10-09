//! Media ingest (media-ingest.md P2.13): declared streams captured into
//! audio segments, transcribed by the queue, text stored in O2.
//!
//! - [`streams`] / [`profiles`]: CRUD services shared by the HTTP
//!   controller (and later the assistant tools, D142 §9.3);
//! - [`MediaIngestSettings`]: platform settings (`settings.media_ingest`,
//!   `PNEX_MEDIA_*` / `PNEX_ASR_*` overrides).

pub mod profiles;
pub mod streams;

use loco_rs::config::Config;
use serde::Deserialize;

/// Platform settings of media ingest.
#[derive(Debug, Clone)]
pub struct MediaIngestSettings {
    /// Max declared streams per org (D159 quota).
    pub max_streams_per_org: u64,
    /// Queue tag of the transcription jobs; empty = untagged queue shared
    /// with firmware and stitch jobs (D166).
    pub asr_queue_tag: String,
}

impl Default for MediaIngestSettings {
    fn default() -> Self {
        Self {
            max_streams_per_org: 3,
            asr_queue_tag: String::new(),
        }
    }
}

#[derive(Debug, Default, Deserialize)]
struct Partial {
    max_streams_per_org: Option<u64>,
    asr_queue_tag: Option<String>,
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
        if let Some(v) = partial.asr_queue_tag {
            out.asr_queue_tag = v;
        }
        if let Some(v) = env_parse::<u64>("PNEX_MEDIA_MAX_STREAMS_PER_ORG") {
            out.max_streams_per_org = v;
        }
        if let Ok(v) = std::env::var("PNEX_ASR_QUEUE_TAG") {
            out.asr_queue_tag = v;
        }
        out.asr_queue_tag = out.asr_queue_tag.trim().to_string();
        out
    }
}

fn env_parse<T: std::str::FromStr>(key: &str) -> Option<T> {
    std::env::var(key).ok().and_then(|v| v.trim().parse().ok())
}
