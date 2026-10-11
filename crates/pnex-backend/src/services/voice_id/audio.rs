//! Audio side of voice identification: an uploaded or stored clip is
//! decoded by the confined ffmpeg of the capture chain (16 kHz mono), cut
//! into windows, and each usable window goes through the org's speaker
//! embedding model, served by a warm `pnex-asr` process (media-ingest.md
//! D166–D167). No vector is ever logged (`WireVoice` prints its dimension
//! only).

use loco_rs::app::AppContext;
use pnex_asr::protocol::{Family, TASK_EMBEDDING};
use pnex_core::vision::ModelCheckStatus;
use pnex_core::voice_id::{self, Window, CLIP_MAX_SECS};
use sea_orm::{ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter, QueryOrder};
use uuid::Uuid;

use super::{Result, VoiceIdError};
use crate::models::_entities::ml_models;
use crate::services::media_ingest::asr::{self, AsrSettings};
use crate::services::media_ingest::capture::{self, segmenter, CaptureError};

/// A decoded clip.
pub struct Clip {
    pub samples: Vec<f32>,
    /// Longer than [`CLIP_MAX_SECS`]: only the start was decoded.
    pub truncated: bool,
}

impl Clip {
    pub fn duration_ms(&self) -> u32 {
        (self.samples.len() * 1000 / voice_id::SAMPLE_RATE) as u32
    }

    pub fn windows(&self) -> Vec<Window> {
        voice_id::windows(&self.samples)
    }
}

/// Decodes any audio the capture chain reads (WAV, MP3, Ogg/Opus, FLAC,
/// M4A…); the demuxer comes from the content type, else the file name.
pub async fn decode(bytes: Vec<u8>, filename: &str, content_type: Option<&str>) -> Result<Clip> {
    let format = capture::decoder::input_format(content_type, filename)
        .ok_or(VoiceIdError::AudioUnsupported)?;
    let (wav, truncated) = capture::decode_clip(bytes, format, CLIP_MAX_SECS)
        .await
        .map_err(|e| match e {
            CaptureError::DecoderMissing => VoiceIdError::Failed("ffmpeg is missing".into()),
            _ => VoiceIdError::AudioUnsupported,
        })?;
    let samples = pnex_asr::read_wav_bytes(&wav).map_err(|_| VoiceIdError::AudioUnsupported)?;
    Ok(Clip { samples, truncated })
}

/// The org's speaker embedding model: `id` when given (it must be one,
/// and valid), else the newest valid one.
pub async fn model(
    db: &DatabaseConnection,
    org_id: i64,
    id: Option<Uuid>,
) -> Result<ml_models::Model> {
    let valid = ModelCheckStatus::Valid.wire();
    let q = ml_models::Entity::find()
        .filter(ml_models::Column::OrgId.eq(org_id))
        .filter(ml_models::Column::Task.eq(TASK_EMBEDDING));
    match id {
        Some(id) => q
            .filter(ml_models::Column::Id.eq(id))
            .one(db)
            .await?
            .filter(|m| m.check_status == valid)
            .ok_or(VoiceIdError::Invalid {
                field: "model_id",
                token: pnex_core::err_codes::FIELD_INVALID.into(),
            }),
        None => q
            .filter(ml_models::Column::CheckStatus.eq(valid))
            .order_by_desc(ml_models::Column::CheckedAt)
            .one(db)
            .await?
            .ok_or(VoiceIdError::NoModel),
    }
}

/// One embedding per window, in order (at most `max`).
pub async fn embed(
    ctx: &AppContext,
    model: &ml_models::Model,
    samples: &[f32],
    windows: &[Window],
    max: usize,
) -> Result<Vec<Vec<f32>>> {
    let failed = |e: String| VoiceIdError::Failed(e);
    let settings = AsrSettings::from_env();
    let local = asr::local_model(ctx, &settings, model.asset_id, model.asset_version)
        .await
        .map_err(|e| failed(e.to_string()))?;
    let launch = asr::launch(&settings, &local, Family::SpeakerEmbedding, "fr")
        .map_err(|e| failed(e.to_string()))?;
    let mut out = Vec::new();
    for w in windows.iter().take(max) {
        let pcm: Vec<i16> = samples[w.start..w.end]
            .iter()
            .map(|x| (x.clamp(-1.0, 1.0) * f32::from(i16::MAX)) as i16)
            .collect();
        let resp =
            asr::process::transcribe(&launch, &settings.sandbox, &segmenter::wav_bytes(&pcm))
                .await
                .map_err(|e| failed(e.to_string()))?;
        let v = resp
            .voices
            .into_iter()
            .next()
            .map(|v| v.v)
            .filter(|v| !v.is_empty())
            .ok_or_else(|| failed("no embedding returned".into()))?;
        out.push(v);
    }
    Ok(out)
}
