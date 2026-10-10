//! ASR profiles (media-ingest.md D164): which registry models a stream
//! transcribes with. Changing a profile only affects new segments.

use pnex_asr::protocol::{TASK_ASR, TASK_EMBEDDING, TASK_SEGMENTATION, TASK_VAD};
use pnex_core::err_codes;
use pnex_core::media_ingest::{is_valid_language, AsrProfile, AsrProfileInput};
use pnex_core::vision::ModelCheckStatus;
use sea_orm::{
    ActiveModelTrait, ColumnTrait, ConnectionTrait, DatabaseConnection, DbErr, EntityTrait,
    QueryFilter, QueryOrder, Set, TransactionTrait,
};
use uuid::Uuid;

use crate::models::_entities::{asr_profiles, ml_models};

const NAME_MAX: usize = 200;
const BEAM_MAX: i32 = 8;

#[derive(Debug, thiserror::Error)]
pub enum ProfileError {
    #[error("invalid {field}: {token}")]
    Invalid { field: &'static str, token: String },
    #[error("ASR profile not found")]
    NotFound,
    #[error("ASR profile name taken")]
    NameTaken,
    #[error(transparent)]
    Db(#[from] DbErr),
}

fn invalid(field: &'static str, token: impl Into<String>) -> ProfileError {
    ProfileError::Invalid {
        field,
        token: token.into(),
    }
}

pub fn view(r: &asr_profiles::Model) -> AsrProfile {
    AsrProfile {
        id: r.id.to_string(),
        name: r.name.clone(),
        asr_model_id: r.asr_model_id.to_string(),
        vad_model_id: r.vad_model_id.map(|v| v.to_string()),
        diarization_model_id: r.diarization_model_id.map(|v| v.to_string()),
        diarization_embedding_model_id: r.diarization_embedding_model_id.map(|v| v.to_string()),
        language: r.language.clone(),
        beam: r.beam,
        word_timestamps: r.word_timestamps,
        created_at: r.created_at.to_rfc3339(),
        updated_at: r.updated_at.to_rfc3339(),
    }
}

pub async fn list<C: ConnectionTrait>(
    db: &C,
    org_id: i64,
) -> Result<Vec<asr_profiles::Model>, DbErr> {
    asr_profiles::Entity::find()
        .filter(asr_profiles::Column::OrgId.eq(org_id))
        .order_by_asc(asr_profiles::Column::Name)
        .all(db)
        .await
}

pub async fn find<C: ConnectionTrait>(
    db: &C,
    org_id: i64,
    id: Uuid,
) -> Result<asr_profiles::Model, ProfileError> {
    asr_profiles::Entity::find_by_id(id)
        .filter(asr_profiles::Column::OrgId.eq(org_id))
        .one(db)
        .await?
        .ok_or(ProfileError::NotFound)
}

/// A model of the org with the expected task that passed its check;
/// anything else is the same field error (no cross-org oracle, R1).
async fn model_of<C: ConnectionTrait>(
    db: &C,
    org_id: i64,
    field: &'static str,
    raw: &str,
    task: &str,
) -> Result<Uuid, ProfileError> {
    let id =
        Uuid::parse_str(raw.trim()).map_err(|_| invalid(field, err_codes::FIELD_INVALID_UUID))?;
    ml_models::Entity::find_by_id(id)
        .filter(ml_models::Column::OrgId.eq(org_id))
        .filter(ml_models::Column::Task.eq(task))
        .filter(ml_models::Column::CheckStatus.eq(ModelCheckStatus::Valid.wire()))
        .one(db)
        .await?
        .map(|m| m.id)
        .ok_or_else(|| invalid(field, err_codes::FIELD_INVALID))
}

async fn name_taken<C: ConnectionTrait>(
    db: &C,
    org_id: i64,
    name: &str,
    except: Option<Uuid>,
) -> Result<bool, DbErr> {
    let mut q = asr_profiles::Entity::find()
        .filter(asr_profiles::Column::OrgId.eq(org_id))
        .filter(asr_profiles::Column::Name.eq(name));
    if let Some(id) = except {
        q = q.filter(asr_profiles::Column::Id.ne(id));
    }
    Ok(q.one(db).await?.is_some())
}

fn clean_name(raw: &str) -> Result<String, ProfileError> {
    let name = raw.trim();
    if name.is_empty() {
        return Err(invalid("name", err_codes::FIELD_REQUIRED));
    }
    if name.chars().count() > NAME_MAX {
        return Err(invalid(
            "name",
            format!("{}:{NAME_MAX}", err_codes::FIELD_MAX_LENGTH),
        ));
    }
    Ok(name.to_string())
}

/// An optional model field: empty clears it, otherwise a checked model of
/// the org with `task`.
async fn optional_model<C: ConnectionTrait>(
    db: &C,
    org_id: i64,
    field: &'static str,
    raw: &str,
    task: &str,
) -> Result<Option<Uuid>, ProfileError> {
    if raw.trim().is_empty() {
        return Ok(None);
    }
    Ok(Some(model_of(db, org_id, field, raw, task).await?))
}

/// Applies `input` on `am` (absent fields keep their value).
async fn apply<C: ConnectionTrait>(
    db: &C,
    org_id: i64,
    am: &mut asr_profiles::ActiveModel,
    input: &AsrProfileInput,
) -> Result<(), ProfileError> {
    if let Some(name) = &input.name {
        am.name = Set(clean_name(name)?);
    }
    if let Some(raw) = &input.asr_model_id {
        am.asr_model_id = Set(model_of(db, org_id, "asr_model_id", raw, TASK_ASR).await?);
    }
    if let Some(raw) = &input.vad_model_id {
        am.vad_model_id = Set(optional_model(db, org_id, "vad_model_id", raw, TASK_VAD).await?);
    }
    if let Some(raw) = &input.diarization_model_id {
        let field = "diarization_model_id";
        am.diarization_model_id =
            Set(optional_model(db, org_id, field, raw, TASK_SEGMENTATION).await?);
    }
    if let Some(raw) = &input.diarization_embedding_model_id {
        let field = "diarization_embedding_model_id";
        am.diarization_embedding_model_id =
            Set(optional_model(db, org_id, field, raw, TASK_EMBEDDING).await?);
    }
    // Diarization runs with both models or not at all.
    let seg = am.diarization_model_id.clone().unwrap().is_some();
    let emb = am.diarization_embedding_model_id.clone().unwrap().is_some();
    if seg && !emb {
        return Err(invalid(
            "diarization_embedding_model_id",
            err_codes::FIELD_REQUIRED,
        ));
    }
    if emb && !seg {
        return Err(invalid("diarization_model_id", err_codes::FIELD_REQUIRED));
    }
    if let Some(lang) = &input.language {
        let lang = lang.trim().to_ascii_lowercase();
        if !is_valid_language(&lang) {
            return Err(invalid("language", err_codes::FIELD_INVALID));
        }
        am.language = Set(lang);
    }
    if let Some(beam) = input.beam {
        if !(1..=BEAM_MAX).contains(&beam) {
            return Err(invalid("beam", err_codes::FIELD_INVALID));
        }
        am.beam = Set(beam);
    }
    if let Some(w) = input.word_timestamps {
        am.word_timestamps = Set(w);
    }
    Ok(())
}

pub async fn create(
    db: &DatabaseConnection,
    org_id: i64,
    input: &AsrProfileInput,
) -> Result<asr_profiles::Model, ProfileError> {
    let name = clean_name(input.name.as_deref().unwrap_or_default())?;
    if input.asr_model_id.is_none() {
        return Err(invalid("asr_model_id", err_codes::FIELD_REQUIRED));
    }
    let txn = db.begin().await?;
    if name_taken(&txn, org_id, &name, None).await? {
        return Err(ProfileError::NameTaken);
    }
    let now: sea_orm::prelude::DateTimeWithTimeZone = chrono::Utc::now().into();
    let mut am = asr_profiles::ActiveModel {
        id: Set(Uuid::new_v4()),
        org_id: Set(org_id),
        name: Set(name),
        vad_model_id: Set(None),
        diarization_model_id: Set(None),
        diarization_embedding_model_id: Set(None),
        language: Set("fr".into()),
        beam: Set(1),
        word_timestamps: Set(true),
        created_at: Set(now),
        updated_at: Set(now),
        ..Default::default()
    };
    apply(&txn, org_id, &mut am, input).await?;
    let row = am.insert(&txn).await?;
    txn.commit().await?;
    Ok(row)
}

pub async fn update(
    db: &DatabaseConnection,
    org_id: i64,
    id: Uuid,
    input: &AsrProfileInput,
) -> Result<asr_profiles::Model, ProfileError> {
    let txn = db.begin().await?;
    let row = find(&txn, org_id, id).await?;
    if let Some(name) = &input.name {
        if name_taken(&txn, org_id, &clean_name(name)?, Some(id)).await? {
            return Err(ProfileError::NameTaken);
        }
    }
    let mut am: asr_profiles::ActiveModel = row.into();
    apply(&txn, org_id, &mut am, input).await?;
    am.updated_at = Set(chrono::Utc::now().into());
    let row = am.update(&txn).await?;
    txn.commit().await?;
    Ok(row)
}

/// Deletes a profile; its streams keep running without one (they stop
/// transcribing until a profile is set, `asr_profile_id` SET NULL).
pub async fn delete(db: &DatabaseConnection, org_id: i64, id: Uuid) -> Result<(), ProfileError> {
    let row = find(db, org_id, id).await?;
    asr_profiles::Entity::delete_by_id(row.id).exec(db).await?;
    Ok(())
}
