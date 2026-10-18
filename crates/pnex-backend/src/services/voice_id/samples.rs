//! Enrolment clips. Each clip is decoded first (unreadable audio is
//! refused before anything is stored); only the chosen span, as decoded
//! (16 kHz mono WAV), is kept as an `audio` media asset of the org so the
//! profile can be generated again with another model.
//! Removing a clip, or the whole profile, deletes the asset and its blobs.

use loco_rs::app::AppContext;
use pnex_core::voice_id::{self as core, CLIP_MAX_SECS, SAMPLE_PENDING};
use sea_orm::{ActiveModelTrait, ColumnTrait, EntityTrait, ModelTrait, QueryFilter, Set};
use uuid::Uuid;

use super::{audio, profiles, Result, VoiceIdError};
use crate::controllers::media::{write_version, IncomingVersion};
use crate::models::_entities::{media_assets, media_versions, voice_samples};
use crate::services::media::MediaSettings;
use crate::services::media_sniff;

/// Media kind of the clips.
pub const AUDIO_KIND: &str = "audio";

/// `m:ss` of a position in seconds.
fn clock(secs: u32) -> String {
    format!("{}:{:02}", secs / 60, secs % 60)
}

/// Adds a clip to the profile: speech is measured now, the embedding at
/// the next generation. `span` = (start, length) in seconds of the part of
/// the recording to enrol.
pub async fn add(
    ctx: &AppContext,
    org_id: i64,
    profile_id: Uuid,
    filename: &str,
    content_type: &str,
    bytes: axum::body::Bytes,
    span: (u32, u32),
) -> Result<voice_samples::Model> {
    if !media_sniff::accepts(AUDIO_KIND, filename, &bytes) {
        return Err(VoiceIdError::AudioUnsupported);
    }
    add_checked(ctx, org_id, profile_id, filename, content_type, bytes, span).await
}

/// [`add`] of bytes whose format is already known: a file of the media
/// library, sniffed at its upload (audio, or the audio track of a video).
async fn add_checked(
    ctx: &AppContext,
    org_id: i64,
    profile_id: Uuid,
    filename: &str,
    content_type: &str,
    bytes: axum::body::Bytes,
    span: (u32, u32),
) -> Result<voice_samples::Model> {
    let profile = profiles::find(&ctx.db, org_id, profile_id).await?;
    let (start, secs) = span;
    let clip =
        audio::decode_span(bytes.to_vec(), filename, Some(content_type), start, secs).await?;
    let usable = core::usable(&clip.windows());
    let person = profiles::person_title(&ctx.db, org_id, profile.object_id).await?;
    let asset = media_assets::ActiveModel {
        id: Set(Uuid::new_v4()),
        org_id: Set(org_id),
        kind: Set(AUDIO_KIND.into()),
        name: Set(format!(
            "{person} — {filename} ({}–{})",
            clock(start),
            clock(start.saturating_add(clip.duration_ms() / 1000))
        )),
        description: Set(Some("Voice profile enrolment clip (consented).".into())),
        metadata: Set(None),
        ..Default::default()
    }
    .insert(&ctx.db)
    .await?;
    let settings = MediaSettings::from_config(&ctx.config);
    if let Err(e) = write_version(
        ctx,
        org_id,
        &asset,
        IncomingVersion {
            filename: format!(
                "{}.wav",
                filename.rsplit_once('.').map_or(filename, |(stem, _)| stem)
            ),
            content_type: "audio/wav".into(),
            note: None,
            bytes: clip.wav.clone().into(),
        },
        &settings,
    )
    .await
    {
        let _ = asset.delete(&ctx.db).await;
        return Err(VoiceIdError::Failed(e.to_string()));
    }
    let row = voice_samples::ActiveModel {
        id: Set(Uuid::new_v4()),
        org_id: Set(org_id),
        profile_id: Set(profile.id),
        media_asset_id: Set(asset.id),
        duration_ms: Set(Some(clip.duration_ms() as i32)),
        speech_ms: Set(Some(usable.iter().map(|w| w.speech_ms).sum::<u32>() as i32)),
        windows: Set(Some(usable.len() as i32)),
        score: Set(None),
        status: Set(SAMPLE_PENDING.into()),
        reason: Set(None),
        created_at: Set(chrono::Utc::now().into()),
    }
    .insert(&ctx.db)
    .await?;
    Ok(row)
}

/// Enrols the passages of a media file confirmed for the profile's person
/// (media-review.md D277): the longest first, [`CLIP_MAX_SECS`] in all,
/// each decoded from the original file and stored like an uploaded clip.
/// The profile exists (its consent was recorded at creation, V3).
pub async fn add_from_media(
    ctx: &AppContext,
    org_id: i64,
    profile_id: Uuid,
    asset_id: Uuid,
) -> Result<Vec<voice_samples::Model>> {
    use pnex_core::media_review::AttributionStatus;
    let profile = profiles::find(&ctx.db, org_id, profile_id).await?;
    let mut spans: Vec<(i64, i64)> =
        crate::services::media_attributions::list(&ctx.db, org_id, asset_id)
            .await?
            .into_iter()
            .filter(|a| {
                a.object_id == profile.object_id && a.status == AttributionStatus::Confirmed
            })
            .map(|a| (a.start_ms, a.end_ms))
            .collect();
    if spans.is_empty() {
        return Err(VoiceIdError::NotEnoughSpeech { found: 0, min: 1 });
    }
    spans.sort_by_key(|(s, e)| std::cmp::Reverse(e - s));
    let asset = media_assets::Entity::find_by_id(asset_id)
        .filter(media_assets::Column::OrgId.eq(org_id))
        .one(&ctx.db)
        .await?
        .ok_or(VoiceIdError::SampleNotFound)?;
    let version = crate::services::vision::asset_version_row(&ctx.db, asset.id, None)
        .await
        .map_err(|e| VoiceIdError::Db(e.to_string()))?
        .ok_or(VoiceIdError::SampleNotFound)?;
    let bytes: axum::body::Bytes = MediaSettings::from_config(&ctx.config)
        .store()
        .map_err(|e| VoiceIdError::Failed(e.to_string()))?
        .get(&version.storage_key)
        .await
        .map_err(|e| VoiceIdError::Failed(e.to_string()))?
        .into();
    let mut left = CLIP_MAX_SECS;
    let mut out = Vec::new();
    for (start, end) in spans {
        if left == 0 {
            break;
        }
        let secs = (((end - start) / 1000) as u32).clamp(1, left);
        left -= secs;
        out.push(
            add_checked(
                ctx,
                org_id,
                profile.id,
                &version.filename,
                &version.content_type,
                bytes.clone(),
                ((start / 1000) as u32, secs),
            )
            .await?,
        );
    }
    Ok(out)
}

pub async fn remove(ctx: &AppContext, org_id: i64, profile_id: Uuid, id: Uuid) -> Result<()> {
    let row = voice_samples::Entity::find_by_id(id)
        .filter(voice_samples::Column::OrgId.eq(org_id))
        .filter(voice_samples::Column::ProfileId.eq(profile_id))
        .one(&ctx.db)
        .await?
        .ok_or(VoiceIdError::SampleNotFound)?;
    let asset = row.media_asset_id;
    row.delete(&ctx.db).await?;
    delete_assets(ctx, org_id, &[asset]).await
}

/// Deletes clip assets with their blobs (the audio is biometric raw data:
/// it goes with the clip, never left behind in the library).
pub async fn delete_assets(ctx: &AppContext, org_id: i64, ids: &[Uuid]) -> Result<()> {
    if ids.is_empty() {
        return Ok(());
    }
    let keys: Vec<String> = media_versions::Entity::find()
        .filter(media_versions::Column::AssetId.is_in(ids.iter().copied()))
        .all(&ctx.db)
        .await?
        .into_iter()
        .map(|v| v.storage_key)
        .collect();
    for id in ids {
        crate::services::resources::purge_for(
            &ctx.db,
            org_id,
            pnex_core::resources::KIND_MEDIA_ASSET,
            &id.to_string(),
        )
        .await
        .map_err(|e| VoiceIdError::Db(e.to_string()))?;
    }
    media_assets::Entity::delete_many()
        .filter(media_assets::Column::OrgId.eq(org_id))
        .filter(media_assets::Column::Id.is_in(ids.iter().copied()))
        .exec(&ctx.db)
        .await?;
    if let Ok(store) = MediaSettings::from_config(&ctx.config).store() {
        for key in keys {
            if let Err(e) = store.delete(&key).await {
                tracing::warn!(error = %e, "voice clip blob not deleted");
            }
        }
    }
    Ok(())
}

/// Bytes, file name and content type of a clip's current version.
pub async fn bytes_of(
    ctx: &AppContext,
    sample: &voice_samples::Model,
) -> Result<(Vec<u8>, String, String)> {
    let version = crate::services::vision::asset_version_row(&ctx.db, sample.media_asset_id, None)
        .await
        .map_err(|e| VoiceIdError::Db(e.to_string()))?
        .ok_or(VoiceIdError::SampleNotFound)?;
    let store = MediaSettings::from_config(&ctx.config)
        .store()
        .map_err(VoiceIdError::Failed)?;
    let bytes = store
        .get(&version.storage_key)
        .await
        .map_err(|e| VoiceIdError::Failed(e.to_string()))?;
    Ok((bytes, version.filename, version.content_type))
}
