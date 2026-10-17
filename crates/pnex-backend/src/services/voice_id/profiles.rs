//! Voice profiles: one per Person object, created only with a recorded
//! consent (K19 variant A). Revoking the consent = deleting the profile,
//! which deletes its clips and their audio at once.

use std::collections::HashMap;

use loco_rs::app::AppContext;
use pnex_core::err_codes::{FIELD_INVALID, FIELD_MAX_LENGTH, FIELD_REQUIRED};
use pnex_core::voice_id::{
    VoiceProfileInput, VoiceProfileView, VoiceSampleView, CONSENT_PROOF_MAX, CONSENT_SCOPE_MAX,
    PERSON_TYPE, STATUS_DRAFT, STATUS_READY,
};
use sea_orm::{
    ActiveModelTrait, ColumnTrait, DatabaseConnection, EntityTrait, IntoActiveModel, ModelTrait,
    QueryFilter, QueryOrder, Set,
};
use uuid::Uuid;

use super::{audio, samples, Result, VoiceIdError};
use crate::models::_entities::{media_assets, ml_models, objects, voice_profiles, voice_samples};

pub async fn find(db: &DatabaseConnection, org_id: i64, id: Uuid) -> Result<voice_profiles::Model> {
    voice_profiles::Entity::find_by_id(id)
        .filter(voice_profiles::Column::OrgId.eq(org_id))
        .one(db)
        .await?
        .ok_or(VoiceIdError::ProfileNotFound)
}

pub async fn find_by_object(
    db: &DatabaseConnection,
    org_id: i64,
    object_id: Uuid,
) -> Result<Option<voice_profiles::Model>> {
    Ok(voice_profiles::Entity::find()
        .filter(voice_profiles::Column::OrgId.eq(org_id))
        .filter(voice_profiles::Column::ObjectId.eq(object_id))
        .one(db)
        .await?)
}

/// A live Person object of the org (archived persons take no new profile).
async fn person(db: &DatabaseConnection, org_id: i64, id: Uuid) -> Result<objects::Model> {
    objects::Entity::find_by_id(id)
        .filter(objects::Column::OrgId.eq(org_id))
        .one(db)
        .await?
        .filter(|o| o.type_key == PERSON_TYPE && o.valid_to.is_none())
        .ok_or(VoiceIdError::NotPerson)
}

pub async fn person_title(db: &DatabaseConnection, org_id: i64, id: Uuid) -> Result<String> {
    Ok(objects::Entity::find_by_id(id)
        .filter(objects::Column::OrgId.eq(org_id))
        .one(db)
        .await?
        .map(|o| o.title)
        .unwrap_or_default())
}

fn invalid(field: &'static str, token: &str) -> VoiceIdError {
    VoiceIdError::Invalid {
        field,
        token: token.to_string(),
    }
}

/// `YYYY-MM-DD` (midnight UTC) or RFC 3339; never in the future.
fn consent_date(s: &str) -> Result<chrono::DateTime<chrono::FixedOffset>> {
    let s = s.trim();
    if s.is_empty() {
        return Err(invalid("consent_at", FIELD_REQUIRED));
    }
    let at = chrono::DateTime::parse_from_rfc3339(s)
        .ok()
        .or_else(|| {
            chrono::NaiveDate::parse_from_str(s, "%Y-%m-%d")
                .ok()
                .and_then(|d| d.and_hms_opt(0, 0, 0))
                .map(|d| d.and_utc().fixed_offset())
        })
        .ok_or_else(|| invalid("consent_at", FIELD_INVALID))?;
    if at > chrono::Utc::now() {
        return Err(invalid("consent_at", FIELD_INVALID));
    }
    Ok(at)
}

fn text(field: &'static str, s: &str, max: usize) -> Result<String> {
    let s = s.trim();
    if s.is_empty() {
        return Err(invalid(field, FIELD_REQUIRED));
    }
    if s.chars().count() > max {
        return Err(invalid(field, &format!("{FIELD_MAX_LENGTH}:{max}")));
    }
    Ok(s.to_string())
}

fn uuid(field: &'static str, s: &str) -> Result<Uuid> {
    if s.trim().is_empty() {
        return Err(invalid(field, FIELD_REQUIRED));
    }
    Uuid::parse_str(s.trim()).map_err(|_| invalid(field, FIELD_INVALID))
}

pub async fn create(
    db: &DatabaseConnection,
    org_id: i64,
    recorded_by: &str,
    input: &VoiceProfileInput,
) -> Result<voice_profiles::Model> {
    if !input.consent_confirmed {
        return Err(invalid("consent_confirmed", FIELD_REQUIRED));
    }
    let consent_at = consent_date(&input.consent_at)?;
    let scope = text("consent_scope", &input.consent_scope, CONSENT_SCOPE_MAX)?;
    let proof = text("consent_proof", &input.consent_proof, CONSENT_PROOF_MAX)?;
    let object = person(db, org_id, uuid("object_id", &input.object_id)?).await?;
    let model_id = match input.model_id.trim() {
        "" => None,
        s => Some(uuid("model_id", s)?),
    };
    let model = audio::model(db, org_id, model_id).await?;
    if find_by_object(db, org_id, object.id).await?.is_some() {
        return Err(VoiceIdError::ProfileExists);
    }
    let now = chrono::Utc::now();
    Ok(voice_profiles::ActiveModel {
        id: Set(Uuid::new_v4()),
        org_id: Set(org_id),
        object_id: Set(object.id),
        model_id: Set(Some(model.id)),
        status: Set(STATUS_DRAFT.into()),
        consent_at: Set(consent_at),
        consent_scope: Set(scope),
        consent_proof: Set(proof),
        consent_recorded_by: Set(recorded_by.to_string()),
        centroid_ciphertext: Set(None),
        centroid_nonce: Set(None),
        centroid_key_id: Set(None),
        dim: Set(None),
        threshold: Set(None),
        windows_used: Set(0),
        intra_mean: Set(None),
        intra_std: Set(None),
        trained_at: Set(None),
        last_error: Set(None),
        created_at: Set(now.into()),
        updated_at: Set(now.into()),
    }
    .insert(db)
    .await?)
}

/// Clears what a generation produced: the profile goes back to `draft`.
pub fn reset(a: &mut voice_profiles::ActiveModel) {
    a.status = Set(STATUS_DRAFT.into());
    a.centroid_ciphertext = Set(None);
    a.centroid_nonce = Set(None);
    a.centroid_key_id = Set(None);
    a.dim = Set(None);
    a.threshold = Set(None);
    a.windows_used = Set(0);
    a.intra_mean = Set(None);
    a.intra_std = Set(None);
    a.trained_at = Set(None);
    a.updated_at = Set(chrono::Utc::now().into());
}

/// Another embedding model: vectors of two models never compare, so the
/// profile must be generated again (the clips are kept).
pub async fn set_model(
    db: &DatabaseConnection,
    org_id: i64,
    id: Uuid,
    model_id: &str,
) -> Result<voice_profiles::Model> {
    let row = find(db, org_id, id).await?;
    let model = audio::model(db, org_id, Some(uuid("model_id", model_id)?)).await?;
    if row.model_id == Some(model.id) {
        return Ok(row);
    }
    let mut a = row.into_active_model();
    reset(&mut a);
    a.model_id = Set(Some(model.id));
    a.last_error = Set(None);
    Ok(a.update(db).await?)
}

/// Consent revoked: the profile, its clips and their audio are deleted.
pub async fn delete(ctx: &AppContext, org_id: i64, id: Uuid) -> Result<()> {
    let row = find(&ctx.db, org_id, id).await?;
    let assets: Vec<Uuid> = voice_samples::Entity::find()
        .filter(voice_samples::Column::ProfileId.eq(row.id))
        .all(&ctx.db)
        .await?
        .into_iter()
        .map(|s| s.media_asset_id)
        .collect();
    row.delete(&ctx.db).await?;
    samples::delete_assets(ctx, org_id, &assets).await
}

pub async fn list(db: &DatabaseConnection, org_id: i64) -> Result<Vec<VoiceProfileView>> {
    let rows = voice_profiles::Entity::find()
        .filter(voice_profiles::Column::OrgId.eq(org_id))
        .order_by_asc(voice_profiles::Column::CreatedAt)
        .all(db)
        .await?;
    views(db, org_id, rows).await
}

pub async fn view(
    db: &DatabaseConnection,
    org_id: i64,
    row: voice_profiles::Model,
) -> Result<VoiceProfileView> {
    Ok(views(db, org_id, vec![row]).await?.remove(0))
}

/// Views with person titles, model names and clips, in four queries.
async fn views(
    db: &DatabaseConnection,
    org_id: i64,
    rows: Vec<voice_profiles::Model>,
) -> Result<Vec<VoiceProfileView>> {
    let ids: Vec<Uuid> = rows.iter().map(|r| r.id).collect();
    let titles: HashMap<Uuid, String> = objects::Entity::find()
        .filter(objects::Column::OrgId.eq(org_id))
        .filter(objects::Column::Id.is_in(rows.iter().map(|r| r.object_id)))
        .all(db)
        .await?
        .into_iter()
        .map(|o| (o.id, o.title))
        .collect();
    let models: HashMap<Uuid, String> = ml_models::Entity::find()
        .filter(ml_models::Column::OrgId.eq(org_id))
        .filter(ml_models::Column::Id.is_in(rows.iter().filter_map(|r| r.model_id)))
        .all(db)
        .await?
        .into_iter()
        .map(|m| (m.id, m.name))
        .collect();
    let clips = voice_samples::Entity::find()
        .filter(voice_samples::Column::ProfileId.is_in(ids))
        .order_by_asc(voice_samples::Column::CreatedAt)
        .all(db)
        .await?;
    let names: HashMap<Uuid, String> = media_assets::Entity::find()
        .filter(media_assets::Column::OrgId.eq(org_id))
        .filter(media_assets::Column::Id.is_in(clips.iter().map(|c| c.media_asset_id)))
        .all(db)
        .await?
        .into_iter()
        .map(|a| (a.id, a.name))
        .collect();
    Ok(rows
        .into_iter()
        .map(|r| VoiceProfileView {
            id: r.id.to_string(),
            object_id: r.object_id.to_string(),
            person: titles.get(&r.object_id).cloned().unwrap_or_default(),
            model_id: r.model_id.map(|m| m.to_string()),
            model_name: r.model_id.and_then(|m| models.get(&m).cloned()),
            status: pnex_core::voice_id::VoiceProfileStatus::from_wire(&r.status),
            threshold: r.threshold,
            windows_used: r.windows_used.max(0) as u32,
            intra_mean: r.intra_mean,
            intra_std: r.intra_std,
            consent_at: r.consent_at.to_rfc3339(),
            consent_scope: r.consent_scope.clone(),
            consent_proof: r.consent_proof.clone(),
            consent_recorded_by: r.consent_recorded_by.clone(),
            trained_at: r.trained_at.map(|t| t.to_rfc3339()),
            last_error: r.last_error.clone(),
            samples: clips
                .iter()
                .filter(|c| c.profile_id == r.id)
                .map(|c| VoiceSampleView {
                    id: c.id.to_string(),
                    media_asset_id: c.media_asset_id.to_string(),
                    name: names.get(&c.media_asset_id).cloned().unwrap_or_default(),
                    duration_ms: c.duration_ms.map(|v| v.max(0) as u32),
                    speech_ms: c.speech_ms.map(|v| v.max(0) as u32),
                    windows: c.windows.map(|v| v.max(0) as u32),
                    score: c.score,
                    status: pnex_core::voice_id::VoiceSampleStatus::from_wire(&c.status),
                    reason: c.reason.clone(),
                    created_at: c.created_at.to_rfc3339(),
                })
                .collect(),
            created_at: r.created_at.to_rfc3339(),
            updated_at: r.updated_at.to_rfc3339(),
        })
        .collect())
}

/// Ready profiles of one embedding model (identification 1:N).
pub async fn ready_with_model(
    db: &DatabaseConnection,
    org_id: i64,
    model_id: Uuid,
) -> Result<Vec<voice_profiles::Model>> {
    Ok(voice_profiles::Entity::find()
        .filter(voice_profiles::Column::OrgId.eq(org_id))
        .filter(voice_profiles::Column::ModelId.eq(model_id))
        .filter(voice_profiles::Column::Status.eq(STATUS_READY))
        .all(db)
        .await?)
}
