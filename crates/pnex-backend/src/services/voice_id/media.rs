//! Voices of a media file matched against the consented voice profiles
//! (media-review.md D275): one voice print per speaker of a transcription
//! run, built from its longest turns in the original file, compared with
//! the ready profiles of the org's embedding model. The result only feeds
//! proposals (`origin = voice`, `status = proposed`): a person confirms.
//!
//! - Only enrolled profiles (consent recorded, V3) are compared; no print
//!   of a speaker is stored (R-P5): the file is decoded again each time.
//! - Every run is written to the `voice_id_usage` journal (V9), scores and
//!   ids only, never a vector.

use loco_rs::app::AppContext;
use pnex_core::media_review::{voice_turns, ReviewSegment};
use pnex_core::voice_id::{self as core, VoiceCandidate, VoiceVerdict, MAX_WINDOWS};
use sea_orm::{ColumnTrait, EntityTrait, QueryFilter};
use uuid::Uuid;

use super::{audio, journal, open, profiles, testing::sealed, Result, VoiceIdError};
use crate::models::_entities::{media_versions, objects};
use crate::services::media::MediaSettings;

/// Windows embedded per speaker (a print = their mean).
const WINDOWS_PER_SPEAKER: usize = 15;

/// A speaker of the run matched to a person.
#[derive(Debug, Clone, PartialEq)]
pub struct SpeakerMatch {
    pub label: String,
    pub object_id: Uuid,
    pub score: f32,
}

/// Matches the speakers of `segments` (a run on `version_id`) against the
/// ready profiles, restricted to `persons` when given (D274).
pub async fn identify(
    ctx: &AppContext,
    org_id: i64,
    version_id: Uuid,
    segments: &[ReviewSegment],
    persons: Option<&[Uuid]>,
    user_id: Option<i64>,
) -> Result<Vec<SpeakerMatch>> {
    let model = audio::model(&ctx.db, org_id, None).await?;
    let mut ready = profiles::ready_with_model(&ctx.db, org_id, model.id).await?;
    if let Some(only) = persons.filter(|p| !p.is_empty()) {
        ready.retain(|p| only.contains(&p.object_id));
    }
    // Each centroid opened once for the whole file.
    let mut candidates = Vec::new();
    for p in &ready {
        let (Some(s), Some(threshold)) = (sealed(p), p.threshold) else {
            continue;
        };
        candidates.push((p, open(ctx, org_id, p.id, &s)?, threshold));
    }
    if candidates.is_empty() {
        return Err(VoiceIdError::NotReady);
    }
    let ids: Vec<Uuid> = candidates.iter().map(|(p, _, _)| p.object_id).collect();
    let names: std::collections::HashMap<Uuid, String> = objects::Entity::find()
        .filter(objects::Column::OrgId.eq(org_id))
        .filter(objects::Column::Id.is_in(ids))
        .all(&ctx.db)
        .await?
        .into_iter()
        .map(|o| (o.id, o.title))
        .collect();
    let version = media_versions::Entity::find_by_id(version_id)
        .filter(media_versions::Column::OrgId.eq(org_id))
        .one(&ctx.db)
        .await?
        .ok_or(VoiceIdError::NotReady)?;
    let store = MediaSettings::from_config(&ctx.config)
        .store()
        .map_err(|e| VoiceIdError::Failed(e.to_string()))?;
    let bytes = store
        .get(&version.storage_key)
        .await
        .map_err(|e| VoiceIdError::Failed(e.to_string()))?;

    let mut labels: Vec<String> = Vec::new();
    for t in segments.iter().flat_map(|s| s.turns.iter()) {
        if !labels.contains(&t.label) {
            labels.push(t.label.clone());
        }
    }
    let mut out = Vec::new();
    let mut journal_rows = Vec::new();
    for label in labels {
        let mut embs: Vec<Vec<f32>> = Vec::new();
        for turn in voice_turns(segments, &label) {
            if embs.len() >= WINDOWS_PER_SPEAKER {
                break;
            }
            let start = (turn.start_ms / 1000) as u32;
            let secs = ((turn.end_ms - turn.start_ms) / 1000).max(1) as u32;
            let clip = audio::decode_span(
                bytes.clone(),
                &version.filename,
                Some(&version.content_type),
                start,
                secs,
            )
            .await?;
            let usable = core::usable(&clip.windows());
            let room = WINDOWS_PER_SPEAKER - embs.len();
            embs.extend(
                audio::embed(ctx, &model, &clip.samples, &usable, room.min(MAX_WINDOWS)).await?,
            );
        }
        let Some(probe) = core::centroid(&embs) else {
            continue;
        };
        let mut scored: Vec<VoiceCandidate> = candidates
            .iter()
            .map(|(p, centroid, threshold)| {
                let score = core::cosine(&probe, centroid);
                VoiceCandidate {
                    profile_id: p.id.to_string(),
                    object_id: p.object_id.to_string(),
                    person: names.get(&p.object_id).cloned().unwrap_or_default(),
                    score,
                    threshold: *threshold,
                    accepted: score >= *threshold,
                }
            })
            .collect();
        scored.sort_by(|a, b| b.score.total_cmp(&a.score));
        let verdict = core::verdict(&scored);
        journal_rows.push(serde_json::json!({
            "label": label,
            "windows": embs.len(),
            "verdict": verdict,
            "best": scored.first().map(|c| serde_json::json!({
                "profile_id": c.profile_id, "object_id": c.object_id, "score": c.score,
            })),
        }));
        if verdict == VoiceVerdict::Match {
            if let Some(best) = scored.first() {
                if let Ok(object_id) = best.object_id.parse() {
                    out.push(SpeakerMatch {
                        label,
                        object_id,
                        score: best.score,
                    });
                }
            }
        }
    }
    journal::record(
        ctx,
        org_id,
        serde_json::json!({
            "action": "identify_media",
            "user_id": user_id,
            "model_id": model.id.to_string(),
            "version_id": version_id.to_string(),
            "speakers": journal_rows,
        }),
    );
    Ok(out)
}
