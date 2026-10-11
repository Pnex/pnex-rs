//! "Generate": every clip of the profile is decoded again, cut into
//! windows, embedded with the profile's model; the core drops the outlier
//! windows (another voice, music), builds the centroid and calibrates the
//! threshold. Each clip gets its score and status, the centroid is sealed.

use loco_rs::app::AppContext;
use pnex_core::voice_id::{
    self as core, TrainError, MAX_WINDOWS, MIN_WINDOWS, SAMPLE_REJECTED, SAMPLE_USED,
    STATUS_FAILED, STATUS_READY,
};
use sea_orm::{
    ActiveModelTrait, ColumnTrait, EntityTrait, IntoActiveModel, QueryFilter, QueryOrder, Set,
};
use uuid::Uuid;

use super::{audio, journal, profiles, samples, Result, VoiceIdError};
use crate::models::_entities::{voice_profiles, voice_samples};

/// Rejection reasons of a clip (machine tokens, translated by the UI).
pub const REASON_NO_SPEECH: &str = "not_enough_speech";
pub const REASON_OTHER_VOICE: &str = "other_voice";
pub const REASON_UNREADABLE: &str = "unreadable";

/// What one clip brought to the generation.
struct ClipRun {
    row: voice_samples::Model,
    /// Index range of its windows in the embedding list.
    range: std::ops::Range<usize>,
    unreadable: bool,
}

pub async fn generate(ctx: &AppContext, org_id: i64, id: Uuid) -> Result<voice_profiles::Model> {
    let profile = profiles::find(&ctx.db, org_id, id).await?;
    let model = audio::model(&ctx.db, org_id, profile.model_id)
        .await
        .map_err(|e| match e {
            VoiceIdError::Invalid { .. } => VoiceIdError::NoModel,
            e => e,
        })?;
    let rows = voice_samples::Entity::find()
        .filter(voice_samples::Column::ProfileId.eq(profile.id))
        .order_by_asc(voice_samples::Column::CreatedAt)
        .all(&ctx.db)
        .await?;
    let mut embeddings: Vec<Vec<f32>> = Vec::new();
    let mut runs = Vec::new();
    for row in rows {
        let start = embeddings.len();
        let room = MAX_WINDOWS.saturating_sub(start);
        let clip = match samples::bytes_of(ctx, &row).await {
            Ok((bytes, name, ct)) => audio::decode(bytes, &name, Some(&ct)).await.ok(),
            Err(_) => None,
        };
        let Some(clip) = clip else {
            runs.push(ClipRun {
                row,
                range: start..start,
                unreadable: true,
            });
            continue;
        };
        let usable = core::usable(&clip.windows());
        if room > 0 && !usable.is_empty() {
            embeddings.extend(audio::embed(ctx, &model, &clip.samples, &usable, room).await?);
        }
        runs.push(ClipRun {
            row,
            range: start..embeddings.len(),
            unreadable: false,
        });
    }
    let trained = core::train(&embeddings);
    for run in &runs {
        let mut a = run.row.clone().into_active_model();
        let (status, score, reason) = match &trained {
            _ if run.unreadable => (SAMPLE_REJECTED, None, Some(REASON_UNREADABLE)),
            _ if run.range.is_empty() => (SAMPLE_REJECTED, None, Some(REASON_NO_SPEECH)),
            Ok(t) => {
                let scores = &t.scores[run.range.clone()];
                let mean = scores.iter().sum::<f32>() / scores.len() as f32;
                if t.used[run.range.clone()].iter().any(|u| *u) {
                    (SAMPLE_USED, Some(mean), None)
                } else {
                    (SAMPLE_REJECTED, Some(mean), Some(REASON_OTHER_VOICE))
                }
            }
            // Nothing generated: the clips stay pending.
            Err(_) => (core::SAMPLE_PENDING, None, None),
        };
        a.status = Set(status.into());
        a.score = Set(score);
        a.reason = Set(reason.map(str::to_string));
        a.windows = Set(Some(run.range.len() as i32));
        a.update(&ctx.db).await?;
    }
    let mut a = profile.clone().into_active_model();
    profiles::reset(&mut a);
    let out = match trained {
        Ok(t) => {
            let sealed = super::seal(ctx, org_id, profile.id, &t.centroid)?;
            a.status = Set(STATUS_READY.into());
            a.centroid_ciphertext = Set(Some(sealed.ciphertext));
            a.centroid_nonce = Set(Some(sealed.nonce));
            a.centroid_key_id = Set(Some(sealed.key_id));
            a.dim = Set(Some(t.centroid.len() as i32));
            a.threshold = Set(Some(t.threshold));
            a.windows_used = Set(t.used.iter().filter(|u| **u).count() as i32);
            a.intra_mean = Set(Some(t.intra_mean));
            a.intra_std = Set(Some(t.intra_std));
            a.trained_at = Set(Some(chrono::Utc::now().into()));
            a.last_error = Set(None);
            Ok(a.update(&ctx.db).await?)
        }
        Err(e) => {
            let found = match e {
                TrainError::NotEnough { usable } => usable,
                TrainError::Degenerate => 0,
            };
            a.status = Set(STATUS_FAILED.into());
            a.last_error = Set(Some(format!("{e:?}")));
            a.update(&ctx.db).await?;
            Err(VoiceIdError::NotEnoughSpeech {
                found,
                min: MIN_WINDOWS,
            })
        }
    };
    journal::record(
        ctx,
        org_id,
        serde_json::json!({
            "action": "generate",
            "profile_id": profile.id.to_string(),
            "object_id": profile.object_id.to_string(),
            "model_id": model.id.to_string(),
            "windows": embeddings.len(),
            "ok": out.is_ok(),
        }),
    );
    out
}
