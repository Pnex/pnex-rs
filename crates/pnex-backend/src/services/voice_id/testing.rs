//! Test of another clip against the profiles: verification 1:1 (one
//! person) or identification 1:N (every ready profile of a model). The
//! tested audio is never stored. The result is a proposal for a human,
//! never written anywhere (K19).

use loco_rs::app::AppContext;
use pnex_core::voice_id::{
    self as core, VoiceCandidate, VoiceTestResult, MAX_WINDOWS, STATUS_READY,
};
use uuid::Uuid;

use super::{audio, journal, profiles, Result, VoiceIdError};
use crate::models::_entities::voice_profiles;
use crate::services::secrets::Sealed;

/// What to compare the clip with.
pub enum Target {
    /// This person's profile only.
    Person(Uuid),
    /// Every ready profile of this model (`None` = newest valid model).
    Model(Option<Uuid>),
}

pub(super) fn sealed(p: &voice_profiles::Model) -> Option<Sealed> {
    Some(Sealed {
        ciphertext: p.centroid_ciphertext.clone()?,
        nonce: p.centroid_nonce.clone()?,
        key_id: p.centroid_key_id.clone()?,
    })
}

pub async fn run(
    ctx: &AppContext,
    org_id: i64,
    user_id: i64,
    target: Target,
    bytes: Vec<u8>,
    filename: &str,
    content_type: Option<&str>,
) -> Result<VoiceTestResult> {
    let (mode, model, candidates) = match target {
        Target::Person(object_id) => {
            let p = profiles::find_by_object(&ctx.db, org_id, object_id)
                .await?
                .ok_or(VoiceIdError::ProfileNotFound)?;
            if p.status != STATUS_READY {
                return Err(VoiceIdError::NotReady);
            }
            let model = audio::model(&ctx.db, org_id, p.model_id)
                .await
                .map_err(|_| VoiceIdError::NotReady)?;
            ("verify", model, vec![p])
        }
        Target::Model(id) => {
            let model = audio::model(&ctx.db, org_id, id).await?;
            let ps = profiles::ready_with_model(&ctx.db, org_id, model.id).await?;
            ("identify", model, ps)
        }
    };
    if candidates.is_empty() {
        return Err(VoiceIdError::NotReady);
    }
    let clip = audio::decode(bytes, filename, content_type).await?;
    let usable = core::usable(&clip.windows());
    if usable.is_empty() {
        return Err(VoiceIdError::NotEnoughSpeech { found: 0, min: 1 });
    }
    let embs = audio::embed(ctx, &model, &clip.samples, &usable, MAX_WINDOWS).await?;
    let probe = core::centroid(&embs).ok_or_else(|| VoiceIdError::Failed("degenerate".into()))?;
    let mut scored = Vec::new();
    for p in &candidates {
        let (Some(s), Some(threshold)) = (sealed(p), p.threshold) else {
            continue;
        };
        let centroid = super::open(ctx, org_id, p.id, &s)?;
        let score = core::cosine(&probe, &centroid);
        scored.push(VoiceCandidate {
            profile_id: p.id.to_string(),
            object_id: p.object_id.to_string(),
            person: profiles::person_title(&ctx.db, org_id, p.object_id).await?,
            score,
            threshold,
            accepted: score >= threshold,
        });
    }
    scored.sort_by(|a, b| b.score.total_cmp(&a.score));
    let verdict = core::verdict(&scored);
    let result = VoiceTestResult {
        model_id: model.id.to_string(),
        audio_ms: clip.duration_ms(),
        speech_ms: usable.iter().map(|w| w.speech_ms).sum(),
        windows: embs.len() as u32,
        verdict,
        best: (verdict == core::VoiceVerdict::Match)
            .then(|| scored.first().cloned())
            .flatten(),
        candidates: scored,
        truncated: clip.truncated,
    };
    journal::record(
        ctx,
        org_id,
        serde_json::json!({
            "action": mode,
            "user_id": user_id,
            "model_id": result.model_id,
            "verdict": verdict,
            "candidates": result.candidates.iter().map(|c| serde_json::json!({
                "profile_id": c.profile_id,
                "object_id": c.object_id,
                "score": c.score,
                "accepted": c.accepted,
            })).collect::<Vec<_>>(),
        }),
    );
    Ok(result)
}
