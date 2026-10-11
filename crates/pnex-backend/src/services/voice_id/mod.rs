//! Voice identification by consented enrolment (voice-id.md, cases.md K19
//! variant A). Kept apart from the diarization of the streams, which stays
//! anonymous (`S1..S16`, media-ingest.md §2): nothing here reads a stream,
//! and no stream label is ever named from a profile.
//!
//! - [`profiles`]: one profile per Person object, created with its consent;
//!   revoking it deletes the profile, its clips and their audio;
//! - [`samples`]: enrolment clips, stored as `audio` media assets;
//! - [`training`]: "Generate" = window embeddings → centroid + calibrated
//!   threshold, sealed with the secrets keyring;
//! - [`testing`]: verification 1:1 or identification 1:N of another clip;
//! - [`audio`]: decoding (confined ffmpeg) and embeddings (`pnex-asr`);
//! - [`journal`]: every generation and test is logged to O2.

pub mod audio;
pub mod journal;
pub mod profiles;
pub mod samples;
pub mod testing;
pub mod training;

use loco_rs::app::AppContext;
use uuid::Uuid;

use crate::services::secrets::crypto::{Keyring, Sealed};

#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum VoiceIdError {
    #[error("not a person object")]
    NotPerson,
    #[error("profile already exists")]
    ProfileExists,
    #[error("profile not found")]
    ProfileNotFound,
    #[error("sample not found")]
    SampleNotFound,
    #[error("no valid speaker embedding model")]
    NoModel,
    #[error("invalid field {field}: {token}")]
    Invalid { field: &'static str, token: String },
    #[error("audio unsupported")]
    AudioUnsupported,
    #[error("not enough speech ({found} < {min})")]
    NotEnoughSpeech { found: usize, min: usize },
    #[error("no generated profile")]
    NotReady,
    #[error("embedding failed: {0}")]
    Failed(String),
    #[error("database: {0}")]
    Db(String),
}

impl From<sea_orm::DbErr> for VoiceIdError {
    fn from(e: sea_orm::DbErr) -> Self {
        Self::Db(e.to_string())
    }
}

pub type Result<T> = std::result::Result<T, VoiceIdError>;

fn keyring(ctx: &AppContext) -> Result<Keyring> {
    Keyring::from_config(&ctx.config).map_err(|e| VoiceIdError::Failed(e.to_string()))
}

/// Seals a centroid for its profile row (AAD = org + profile id: a row
/// copied onto another profile or org fails to open).
pub fn seal(ctx: &AppContext, org_id: i64, profile_id: Uuid, v: &[f32]) -> Result<Sealed> {
    let json = serde_json::to_string(v).map_err(|e| VoiceIdError::Failed(e.to_string()))?;
    Ok(keyring(ctx)?.seal(Some(org_id), profile_id, &json))
}

pub fn open(ctx: &AppContext, org_id: i64, profile_id: Uuid, sealed: &Sealed) -> Result<Vec<f32>> {
    let json = keyring(ctx)?
        .open(Some(org_id), profile_id, sealed)
        .map_err(|e| VoiceIdError::Failed(e.to_string()))?;
    serde_json::from_str(&json).map_err(|e| VoiceIdError::Failed(e.to_string()))
}
