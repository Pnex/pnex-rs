//! Enrolment clips. Each clip is decoded first (unreadable audio is
//! refused before anything is stored), then kept as an `audio` media asset
//! of the org so the profile can be generated again with another model.
//! Removing a clip, or the whole profile, deletes the asset and its blobs.

use loco_rs::app::AppContext;
use pnex_core::voice_id::{self as core, SAMPLE_PENDING};
use sea_orm::{ActiveModelTrait, ColumnTrait, EntityTrait, ModelTrait, QueryFilter, Set};
use uuid::Uuid;

use super::{audio, profiles, Result, VoiceIdError};
use crate::controllers::media::{write_version, IncomingVersion};
use crate::models::_entities::{media_assets, media_versions, voice_samples};
use crate::services::media::MediaSettings;
use crate::services::media_sniff;

/// Media kind of the clips.
pub const AUDIO_KIND: &str = "audio";

/// Adds a clip to the profile: speech is measured now, the embedding at
/// the next generation.
pub async fn add(
    ctx: &AppContext,
    org_id: i64,
    profile_id: Uuid,
    filename: &str,
    content_type: &str,
    bytes: axum::body::Bytes,
) -> Result<voice_samples::Model> {
    let profile = profiles::find(&ctx.db, org_id, profile_id).await?;
    if !media_sniff::accepts(AUDIO_KIND, filename, &bytes) {
        return Err(VoiceIdError::AudioUnsupported);
    }
    let clip = audio::decode(bytes.to_vec(), filename, Some(content_type)).await?;
    let usable = core::usable(&clip.windows());
    let person = profiles::person_title(&ctx.db, org_id, profile.object_id).await?;
    let asset = media_assets::ActiveModel {
        id: Set(Uuid::new_v4()),
        org_id: Set(org_id),
        kind: Set(AUDIO_KIND.into()),
        name: Set(format!("{person} — {filename}")),
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
            filename: filename.to_string(),
            content_type: content_type.to_string(),
            note: None,
            bytes,
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
