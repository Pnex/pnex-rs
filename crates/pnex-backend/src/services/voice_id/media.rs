//! Voices of a media file matched against the consented voice profiles
//! (media.md D275), prints first (media.md D978–D985): every turn
//! of a transcription run is cut in short pieces, each piece is matched
//! against all the ready profiles of the org's embedding model, so several
//! people merged under one diarization label are still told apart (AMI:
//! speaker confusion halved). The result only feeds proposals
//! (`origin = voice`, `status = proposed`): a person confirms.
//!
//! - Only enrolled profiles (consent recorded, V3) are compared; no print
//!   of a speaker is stored (R-P5): the file is decoded again each time.
//! - Every run is written to the `voice_id_usage` journal (V9), scores and
//!   ids only, never a vector.

use loco_rs::app::AppContext;
use pnex_core::media_review::ReviewSegment;
use pnex_core::voice_id::{self as core};
use pnex_core::voice_recognition::{
    pieces as voice_pieces, recognize, Piece, Print, Who, BRIDGE_GAP_MS, PIECE_MIN_MS,
};
use sea_orm::{ColumnTrait, EntityTrait, QueryFilter};
use uuid::Uuid;

use super::{audio, journal, open, profiles, testing::sealed, Result, VoiceIdError};
use crate::models::_entities::media_versions;
use crate::services::media::MediaSettings;

/// A stretch of the media where a person was recognized.
#[derive(Debug, Clone, PartialEq)]
pub struct VoiceSpan {
    pub object_id: Uuid,
    pub start_ms: i64,
    pub end_ms: i64,
    /// Mean similarity of its pieces with the person's print.
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
) -> Result<Vec<VoiceSpan>> {
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

    let prints: Vec<Print> = candidates
        .iter()
        .map(|(_, centroid, threshold)| Print {
            centroid: centroid.clone(),
            threshold: *threshold,
        })
        .collect();
    // Pieces of every turn, embedded segment by segment (the file is
    // decoded again each time, no print of a speaker is stored, R-P5).
    let mut pieces: Vec<Piece> = Vec::new();
    for (si, seg) in segments.iter().enumerate() {
        if seg.turns.is_empty() {
            continue;
        }
        let start_s = (seg.start_ms.max(0) / 1000) as u32;
        let secs = ((seg.end_ms - seg.start_ms).max(0) as u32 / 1000 + 2).min(core::CLIP_MAX_SECS);
        let clip = audio::decode_span(
            bytes.clone(),
            &version.filename,
            Some(&version.content_type),
            start_s,
            secs,
        )
        .await?;
        let origin = i64::from(start_s) * 1000;
        let at = |ms: i64| {
            (((ms - origin).max(0) as usize) * core::SAMPLE_RATE / 1000).min(clip.samples.len())
        };
        let mut seg_pieces = Vec::new();
        for turn in &seg.turns {
            let group = Some(((si as u64) << 32) | u64::from(label_hash(&turn.label)));
            let span = (turn.start_ms.max(0) as u32, turn.end_ms.max(0) as u32);
            for (s, e) in voice_pieces(&[span]) {
                seg_pieces.push(Piece {
                    start_ms: s,
                    end_ms: e,
                    v: None,
                    group,
                });
            }
        }
        let long: Vec<usize> = (0..seg_pieces.len())
            .filter(|&i| seg_pieces[i].end_ms - seg_pieces[i].start_ms >= PIECE_MIN_MS)
            .filter(|&i| {
                at(i64::from(seg_pieces[i].end_ms)) > at(i64::from(seg_pieces[i].start_ms))
            })
            .collect();
        let windows: Vec<core::Window> = long
            .iter()
            .map(|&i| core::Window {
                start: at(i64::from(seg_pieces[i].start_ms)),
                end: at(i64::from(seg_pieces[i].end_ms)),
                speech_ms: seg_pieces[i].end_ms - seg_pieces[i].start_ms,
            })
            .collect();
        let embs = audio::embed(ctx, &model, &clip.samples, &windows, windows.len()).await?;
        for (&i, v) in long.iter().zip(embs) {
            seg_pieces[i].v = Some(v);
        }
        pieces.extend(seg_pieces);
    }
    let who = recognize(&pieces, &prints);
    // Consecutive pieces of one person become one span.
    let mut order: Vec<usize> = (0..pieces.len()).collect();
    order.sort_by_key(|&i| pieces[i].start_ms);
    let mut out: Vec<VoiceSpan> = Vec::new();
    let mut scores: Vec<Vec<f32>> = Vec::new();
    let mut per_person: std::collections::HashMap<Uuid, u32> = Default::default();
    for i in order {
        let Who::Person(k) = who[i] else { continue };
        let object_id = candidates[k].0.object_id;
        let (s, e) = (i64::from(pieces[i].start_ms), i64::from(pieces[i].end_ms));
        let score = pieces[i]
            .v
            .as_deref()
            .map(|v| core::cosine(v, &prints[k].centroid));
        *per_person.entry(object_id).or_default() += pieces[i].end_ms - pieces[i].start_ms;
        match out.last_mut() {
            Some(last)
                if last.object_id == object_id && s - last.end_ms <= i64::from(BRIDGE_GAP_MS) =>
            {
                last.end_ms = last.end_ms.max(e);
                scores.last_mut().unwrap().extend(score);
            }
            _ => {
                out.push(VoiceSpan {
                    object_id,
                    start_ms: s,
                    end_ms: e,
                    score: 0.0,
                });
                scores.push(score.into_iter().collect());
            }
        }
    }
    for (span, sc) in out.iter_mut().zip(&scores) {
        span.score = if sc.is_empty() {
            0.0
        } else {
            sc.iter().sum::<f32>() / sc.len() as f32
        };
    }
    let unknown_ms: u32 = pieces
        .iter()
        .zip(&who)
        .filter(|(_, w)| matches!(w, Who::Unknown(_)))
        .map(|(p, _)| p.end_ms - p.start_ms)
        .sum();
    journal::record(
        ctx,
        org_id,
        serde_json::json!({
            "action": "identify_media",
            "method": "prints_first",
            "user_id": user_id,
            "model_id": model.id.to_string(),
            "version_id": version_id.to_string(),
            "pieces": pieces.len(),
            "unknown_ms": unknown_ms,
            "persons": per_person
                .iter()
                .map(|(id, ms)| serde_json::json!({ "object_id": id.to_string(), "speech_ms": ms }))
                .collect::<Vec<_>>(),
        }),
    );
    Ok(out)
}

/// Stable small key of a diarization label within a segment.
fn label_hash(label: &str) -> u32 {
    label.bytes().fold(2_166_136_261u32, |h, b| {
        (h ^ u32::from(b)).wrapping_mul(16_777_619)
    })
}
