//! Audio models of the registry (media-ingest.md D167): `ml_models` rows of
//! tasks `asr`, `vad`, `diarization_segmentation` and `speaker_embedding`,
//! next to the vision ones. What the files dictate (family,
//! runtime) is read, never typed; the check measures load time, real-time
//! factor and WER on the reference sample.

use loco_rs::app::AppContext;
use pnex_core::err_codes;
use pnex_core::media_ingest::{AsrModel, AsrModelCheck, AsrModelInput, ASR_LICENSES};
use pnex_core::vision::ModelCheckStatus;
use sea_orm::{
    ActiveModelTrait, ColumnTrait, ConnectionTrait, DbErr, EntityTrait, QueryFilter, QueryOrder,
    Set,
};
use uuid::Uuid;

use pnex_asr::protocol::AUDIO_TASKS;

use super::asr::{self, AsrError, AsrSettings};
use crate::models::_entities::{media_assets, ml_models};

const NAME_MAX: usize = 200;
const DESCRIPTION_MAX: usize = 2000;

#[derive(Debug, thiserror::Error)]
pub enum ModelError {
    #[error("invalid {field}: {token}")]
    Invalid { field: &'static str, token: String },
    #[error("audio model not found")]
    NotFound,
    #[error("model name taken")]
    NameTaken,
    #[error("not a supported audio model")]
    Unsupported,
    #[error(transparent)]
    Db(#[from] DbErr),
}

fn invalid(field: &'static str, token: impl Into<String>) -> ModelError {
    ModelError::Invalid {
        field,
        token: token.into(),
    }
}

fn meta_str(m: &ml_models::Model, key: &str) -> Option<String> {
    m.audio_meta
        .as_ref()
        .and_then(|v| v.get(key))
        .and_then(|v| v.as_str())
        .map(str::to_string)
}

fn meta_f64(m: &ml_models::Model, key: &str) -> Option<f64> {
    m.audio_meta
        .as_ref()
        .and_then(|v| v.get(key))
        .and_then(|v| v.as_f64())
}

pub fn view(m: &ml_models::Model) -> AsrModel {
    AsrModel {
        id: m.id.to_string(),
        name: m.name.clone(),
        description: m.description.clone().unwrap_or_default(),
        task: m.task.clone(),
        family: m.family.clone(),
        asset_id: m.asset_id.to_string(),
        asset_version: m.asset_version,
        license: meta_str(m, "license").unwrap_or_default(),
        check: AsrModelCheck {
            status: m.check_status.clone(),
            error: m.check_error.clone(),
            load_ms: meta_f64(m, "load_ms").map(|v| v as u64),
            rtf: meta_f64(m, "rtf"),
            wer: meta_f64(m, "wer"),
            checked_at: m.checked_at.map(|t| t.to_rfc3339()),
        },
        carriers: Vec::new(),
        created_at: m.created_at.to_rfc3339(),
        updated_at: m.updated_at.to_rfc3339(),
    }
}

/// [`view`] with the per-carrier checks of the models (one query).
pub async fn views<C: ConnectionTrait>(
    db: &C,
    rows: &[ml_models::Model],
) -> Result<Vec<AsrModel>, DbErr> {
    use crate::models::_entities::ml_model_checks;
    let ids: Vec<Uuid> = rows.iter().map(|m| m.id).collect();
    let checks = ml_model_checks::Entity::find()
        .filter(ml_model_checks::Column::ModelId.is_in(ids))
        .order_by_asc(ml_model_checks::Column::Carrier)
        .all(db)
        .await?;
    Ok(rows
        .iter()
        .map(|m| {
            let mut v = view(m);
            v.carriers = checks
                .iter()
                .filter(|c| c.model_id == m.id)
                .map(|c| pnex_core::media_ingest::AsrCarrierCheck {
                    carrier: c.carrier.clone(),
                    check: AsrModelCheck {
                        status: c.check_status.clone(),
                        error: c.check_error.clone(),
                        load_ms: c.load_ms.map(|v| v as u64),
                        rtf: c.rtf,
                        wer: c.wer,
                        checked_at: Some(c.checked_at.to_rfc3339()),
                    },
                })
                .collect();
            v
        })
        .collect())
}

/// Stores the outcome of a check run on `carrier` (insert or replace).
pub async fn save_carrier_check<C: ConnectionTrait>(
    db: &C,
    model: &ml_models::Model,
    carrier: &str,
    report: &Result<asr::CheckReport, AsrError>,
) -> Result<(), DbErr> {
    use crate::models::_entities::ml_model_checks;
    use sea_orm::sea_query::OnConflict;
    let (status, error, load_ms, rtf, wer) = match report {
        Ok(r) => (
            ModelCheckStatus::Valid,
            None,
            Some(r.load_ms as i64),
            Some(r.rtf),
            r.wer,
        ),
        Err(e) => (
            ModelCheckStatus::Invalid,
            Some(e.to_string()),
            None,
            None,
            None,
        ),
    };
    let row = ml_model_checks::ActiveModel {
        id: Set(Uuid::new_v4()),
        org_id: Set(model.org_id),
        model_id: Set(model.id),
        carrier: Set(carrier.chars().take(64).collect()),
        check_status: Set(status.wire().into()),
        check_error: Set(error),
        load_ms: Set(load_ms),
        rtf: Set(rtf),
        wer: Set(wer),
        checked_at: Set(chrono::Utc::now().into()),
    };
    ml_model_checks::Entity::insert(row)
        .on_conflict(
            OnConflict::columns([
                ml_model_checks::Column::ModelId,
                ml_model_checks::Column::Carrier,
            ])
            .update_columns([
                ml_model_checks::Column::CheckStatus,
                ml_model_checks::Column::CheckError,
                ml_model_checks::Column::LoadMs,
                ml_model_checks::Column::Rtf,
                ml_model_checks::Column::Wer,
                ml_model_checks::Column::CheckedAt,
            ])
            .to_owned(),
        )
        .exec(db)
        .await?;
    Ok(())
}

pub async fn list<C: ConnectionTrait>(db: &C, org_id: i64) -> Result<Vec<ml_models::Model>, DbErr> {
    ml_models::Entity::find()
        .filter(ml_models::Column::OrgId.eq(org_id))
        .filter(ml_models::Column::Task.is_in(AUDIO_TASKS))
        .order_by_asc(ml_models::Column::Name)
        .all(db)
        .await
}

pub async fn find<C: ConnectionTrait>(
    db: &C,
    org_id: i64,
    id: Uuid,
) -> Result<ml_models::Model, ModelError> {
    ml_models::Entity::find_by_id(id)
        .filter(ml_models::Column::OrgId.eq(org_id))
        .filter(ml_models::Column::Task.is_in(AUDIO_TASKS))
        .one(db)
        .await?
        .ok_or(ModelError::NotFound)
}

fn clean_name(raw: &str) -> Result<String, ModelError> {
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

fn clean_description(raw: &str) -> Result<Option<String>, ModelError> {
    let d = raw.trim();
    if d.chars().count() > DESCRIPTION_MAX {
        return Err(invalid(
            "description",
            format!("{}:{DESCRIPTION_MAX}", err_codes::FIELD_MAX_LENGTH),
        ));
    }
    Ok((!d.is_empty()).then(|| d.to_string()))
}

fn clean_license(raw: &str) -> Result<String, ModelError> {
    let l = raw.trim();
    if l.is_empty() {
        return Err(invalid("license", err_codes::FIELD_REQUIRED));
    }
    if !ASR_LICENSES.contains(&l) {
        return Err(invalid("license", err_codes::FIELD_INVALID));
    }
    Ok(l.to_string())
}

async fn name_taken<C: ConnectionTrait>(
    db: &C,
    org_id: i64,
    name: &str,
    except: Option<Uuid>,
) -> Result<bool, DbErr> {
    // The unique index spans vision and audio models.
    let mut q = ml_models::Entity::find()
        .filter(ml_models::Column::OrgId.eq(org_id))
        .filter(ml_models::Column::Name.eq(name));
    if let Some(id) = except {
        q = q.filter(ml_models::Column::Id.ne(id));
    }
    Ok(q.one(db).await?.is_some())
}

/// Runs the check and stores its outcome (status, measures, diagnostic).
/// A runtime failure is a check result (`invalid` + reason), not an error.
pub async fn run_check(
    ctx: &AppContext,
    row: ml_models::Model,
) -> Result<ml_models::Model, ModelError> {
    let report = asr::check(ctx, &row).await;
    let mut meta = row
        .audio_meta
        .clone()
        .unwrap_or_else(|| serde_json::json!({}));
    let now: sea_orm::prelude::DateTimeWithTimeZone = chrono::Utc::now().into();
    let mut am: ml_models::ActiveModel = row.into();
    match report {
        Ok(r) => {
            if let Some(obj) = meta.as_object_mut() {
                for (k, v) in [
                    ("runtime", serde_json::json!(r.runtime)),
                    ("engine", serde_json::json!(r.engine)),
                    ("load_ms", serde_json::json!(r.load_ms)),
                    ("rtf", serde_json::json!(r.rtf)),
                    ("wer", serde_json::json!(r.wer)),
                    ("check_transcript", serde_json::json!(r.transcript)),
                    ("speech_ms", serde_json::json!(r.speech_ms)),
                ] {
                    obj.insert(k.into(), v);
                }
            }
            am.family = Set(r.family);
            am.check_status = Set(ModelCheckStatus::Valid.wire().into());
            am.check_error = Set(None);
            am.infer_ms = Set(Some(r.infer_ms as i64));
        }
        Err(AsrError::Archive(_) | AsrError::Unsupported(_) | AsrError::MissingBytes) => {
            return Err(ModelError::Unsupported)
        }
        Err(e) => {
            am.check_status = Set(ModelCheckStatus::Invalid.wire().into());
            am.check_error = Set(Some(e.to_string()));
        }
    }
    am.audio_meta = Set(Some(meta));
    am.checked_at = Set(Some(now));
    let row = am.update(&ctx.db).await?;
    // A dedicated transcription queue: check where the model will run too.
    if !super::asr_queue_tags().is_empty() {
        use crate::workers::check_asr_model::{CheckAsrModelArgs, CheckAsrModelWorker};
        use loco_rs::bgworker::BackgroundWorker;
        let args = CheckAsrModelArgs {
            model_id: row.id,
            org_id: row.org_id,
        };
        if let Err(e) = CheckAsrModelWorker::perform_later(ctx, args).await {
            tracing::warn!(model = %row.id, error = %e, "worker model check not enqueued");
        }
    }
    Ok(row)
}

pub async fn create(
    ctx: &AppContext,
    org_id: i64,
    input: &AsrModelInput,
) -> Result<ml_models::Model, ModelError> {
    let name = clean_name(input.name.as_deref().unwrap_or_default())?;
    let description = clean_description(input.description.as_deref().unwrap_or_default())?;
    let license = clean_license(input.license.as_deref().unwrap_or_default())?;
    let asset_id = input
        .asset_id
        .as_deref()
        .ok_or_else(|| invalid("asset_id", err_codes::FIELD_REQUIRED))
        .and_then(|raw| {
            Uuid::parse_str(raw.trim())
                .map_err(|_| invalid("asset_id", err_codes::FIELD_INVALID_UUID))
        })?;
    let asset_ok = media_assets::Entity::find_by_id(asset_id)
        .filter(media_assets::Column::OrgId.eq(org_id))
        .one(&ctx.db)
        .await?
        .is_some_and(|a| a.kind == "model");
    if !asset_ok {
        return Err(invalid("asset_id", err_codes::FIELD_INVALID));
    }
    if name_taken(&ctx.db, org_id, &name, None).await? {
        return Err(ModelError::NameTaken);
    }
    // Read the files before anything is stored: an archive that is not a
    // model is refused outright.
    let settings = AsrSettings::from_env();
    let local = match asr::local_model(ctx, &settings, asset_id, input.asset_version).await {
        Ok(l) => l,
        Err(AsrError::Internal(e)) => return Err(ModelError::Db(DbErr::Custom(e))),
        Err(_) => return Err(ModelError::Unsupported),
    };
    let now: sea_orm::prelude::DateTimeWithTimeZone = chrono::Utc::now().into();
    let row = ml_models::ActiveModel {
        id: Set(Uuid::new_v4()),
        org_id: Set(org_id),
        name: Set(name),
        description: Set(description),
        task: Set(local.inspection.family.task().into()),
        family: Set(local.inspection.family.wire().into()),
        asset_id: Set(asset_id),
        asset_version: Set(input.asset_version),
        input_width: Set(0),
        input_height: Set(0),
        labels: Set(serde_json::json!([])),
        score_threshold: Set(0.0),
        nms_iou: Set(0.0),
        check_status: Set(ModelCheckStatus::Unchecked.wire().into()),
        check_error: Set(None),
        infer_ms: Set(None),
        checked_at: Set(None),
        audio_meta: Set(Some(serde_json::json!({
            "license": license,
            "files": local.inspection.files,
        }))),
        created_at: Set(now),
        updated_at: Set(now),
    }
    .insert(&ctx.db)
    .await?;
    run_check(ctx, row).await
}

/// Renames / re-describes / re-licenses; the file never changes (import a
/// new model instead).
pub async fn update(
    ctx: &AppContext,
    org_id: i64,
    id: Uuid,
    input: &AsrModelInput,
) -> Result<ml_models::Model, ModelError> {
    let row = find(&ctx.db, org_id, id).await?;
    let mut meta = row
        .audio_meta
        .clone()
        .unwrap_or_else(|| serde_json::json!({}));
    let mut am: ml_models::ActiveModel = row.clone().into();
    if let Some(name) = &input.name {
        let name = clean_name(name)?;
        if name_taken(&ctx.db, org_id, &name, Some(id)).await? {
            return Err(ModelError::NameTaken);
        }
        am.name = Set(name);
    }
    if let Some(d) = &input.description {
        am.description = Set(clean_description(d)?);
    }
    if let Some(l) = &input.license {
        let l = clean_license(l)?;
        if let Some(obj) = meta.as_object_mut() {
            obj.insert("license".into(), serde_json::json!(l));
        }
        am.audio_meta = Set(Some(meta));
    }
    am.updated_at = Set(chrono::Utc::now().into());
    Ok(am.update(&ctx.db).await?)
}

/// Deletes a model. Profiles using it go with it (cascade); their streams
/// stop transcribing until another profile is set.
pub async fn delete(ctx: &AppContext, org_id: i64, id: Uuid) -> Result<(), ModelError> {
    let row = find(&ctx.db, org_id, id).await?;
    ml_models::Entity::delete_by_id(row.id)
        .exec(&ctx.db)
        .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn license_must_be_listed() {
        assert!(clean_license("Apache-2.0").is_ok());
        assert!(matches!(
            clean_license(""),
            Err(ModelError::Invalid {
                field: "license",
                ..
            })
        ));
        assert!(matches!(
            clean_license("WTFPL"),
            Err(ModelError::Invalid {
                field: "license",
                ..
            })
        ));
    }
}
