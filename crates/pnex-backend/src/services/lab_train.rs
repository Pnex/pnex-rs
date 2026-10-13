//! Vision Lab training (media-vision-studio.md D204): fine-tunes the head
//! of a registered YOLOX model on a frozen dataset version, pure Rust
//! (`pnex_vision::train`), in the API process.
//!
//! - One training at a time per process ([`GATE`]); a queued run waits.
//! - Liveness by heartbeat: the runner refreshes `heartbeat_at` every
//!   [`HEARTBEAT`]; a queued or running run whose heartbeat is older than
//!   [`STALE_SECS`] (process stopped, pod gone) is closed as
//!   `lab-train-interrupted` the next time its project's runs are read.
//! - Cancellation: the API flips the row to `cancelled`; the runner sees
//!   it at its next progress tick and stops the trainer.
//! - Success: the ONNX becomes a `model` asset of the media library and a
//!   registry entry checked like any upload (D100), labels = classes.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use axum::http::StatusCode;
use pnex_core::err_codes;
use pnex_core::lab::{
    DatasetSnapshot, LabRunProgress, LabRunStatus, LabSplit, LabTrainRequest, LabTrainingRun,
};
use pnex_core::vision::{ModelSpec, VisionFamily};
use pnex_vision::train::{Progress, TrainError, TrainImage};
use sea_orm::sea_query::Expr;
use sea_orm::{
    ActiveModelTrait, ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter, QueryOrder, Set,
};
use uuid::Uuid;

use crate::models::_entities::{
    lab_dataset_versions, lab_training_runs, media_assets, media_versions, ml_models,
};
use crate::services::lab::{find_project, not_found, LabError, Res, EXPORT_MAX_IMAGES};
use crate::services::media::MediaSettings;
use crate::services::vision;

const HEARTBEAT: Duration = Duration::from_secs(10);
pub const STALE_SECS: i64 = 60;
/// Progress written at most this often.
const TICK: Duration = Duration::from_secs(2);

// shortcut: in-process, one run per pod; move to the worker fabric (P2.7)
// when trainings must leave the API pods.
static GATE: tokio::sync::Semaphore = tokio::sync::Semaphore::const_new(1);

fn now() -> chrono::DateTime<chrono::FixedOffset> {
    chrono::Utc::now().into()
}

fn view(r: &lab_training_runs::Model, version: i32, base_name: String) -> LabTrainingRun {
    LabTrainingRun {
        id: r.id.to_string(),
        dataset_version_id: r.dataset_version_id.to_string(),
        dataset_version: version,
        base_model_id: r.base_model_id.map(|u| u.to_string()),
        base_model_name: base_name,
        status: LabRunStatus::from_wire(&r.status),
        epochs: r.epochs.max(0) as u32,
        progress: serde_json::from_value(r.progress.clone()).unwrap_or_default(),
        map50: r.map50,
        error: r.error.clone(),
        error_detail: r.error_detail.clone(),
        model_id: r.model_id.map(|u| u.to_string()),
        created_at: r.created_at.to_rfc3339(),
        finished_at: r.finished_at.map(|t| t.to_rfc3339()),
    }
}

/// Closes the project's runs whose runner stopped beating.
async fn reap(db: &DatabaseConnection, project: Uuid) -> Res<()> {
    let limit = now() - chrono::Duration::seconds(STALE_SECS);
    lab_training_runs::Entity::update_many()
        .col_expr(lab_training_runs::Column::Status, Expr::value("failed"))
        .col_expr(
            lab_training_runs::Column::Error,
            Expr::value(err_codes::LAB_TRAIN_INTERRUPTED),
        )
        .col_expr(lab_training_runs::Column::FinishedAt, Expr::value(now()))
        .filter(lab_training_runs::Column::ProjectId.eq(project))
        .filter(lab_training_runs::Column::Status.is_in(["queued", "running"]))
        .filter(lab_training_runs::Column::HeartbeatAt.lt(limit))
        .exec(db)
        .await
        .map_err(LabError::db)?;
    Ok(())
}

async fn views(
    db: &DatabaseConnection,
    rows: Vec<lab_training_runs::Model>,
) -> Res<Vec<LabTrainingRun>> {
    let mut out = Vec::with_capacity(rows.len());
    for r in rows {
        let version = lab_dataset_versions::Entity::find_by_id(r.dataset_version_id)
            .one(db)
            .await
            .map_err(LabError::db)?
            .map_or(0, |v| v.version);
        let base = match r.base_model_id {
            Some(id) => ml_models::Entity::find_by_id(id)
                .one(db)
                .await
                .map_err(LabError::db)?
                .map(|m| m.name)
                .unwrap_or_default(),
            None => String::new(),
        };
        out.push(view(&r, version, base));
    }
    Ok(out)
}

/// Runs of a project, newest first (stale ones closed on the way).
pub async fn list(db: &DatabaseConnection, org_id: i64, project: Uuid) -> Res<Vec<LabTrainingRun>> {
    find_project(db, org_id, project).await?;
    reap(db, project).await?;
    let rows = lab_training_runs::Entity::find()
        .filter(lab_training_runs::Column::ProjectId.eq(project))
        .order_by_desc(lab_training_runs::Column::CreatedAt)
        .all(db)
        .await
        .map_err(LabError::db)?;
    views(db, rows).await
}

/// Queues a training of `version` and starts its runner.
pub async fn start(
    ctx: &loco_rs::app::AppContext,
    org_id: i64,
    version_id: Uuid,
    req: &LabTrainRequest,
    user: &str,
) -> Res<LabTrainingRun> {
    let db = &ctx.db;
    if let Some((field, token)) = req.check() {
        return Err(LabError::fields(vec![(field, token)]));
    }
    let version = lab_dataset_versions::Entity::find_by_id(version_id)
        .filter(lab_dataset_versions::Column::OrgId.eq(org_id))
        .one(db)
        .await
        .map_err(LabError::db)?
        .ok_or_else(|| not_found(err_codes::LAB_VERSION_NOT_FOUND, "Dataset version"))?;
    let base_field = || {
        LabError::fields(vec![(
            "base_model_id",
            err_codes::FIELD_INVALID.to_string(),
        )])
    };
    let base_id = Uuid::parse_str(&req.base_model_id).map_err(|_| base_field())?;
    let base = ml_models::Entity::find_by_id(base_id)
        .filter(ml_models::Column::OrgId.eq(org_id))
        .filter(ml_models::Column::Task.eq(vision::VISION_TASK))
        .one(db)
        .await
        .map_err(LabError::db)?
        .ok_or_else(base_field)?;
    if base.family != VisionFamily::Yolox.wire() {
        return Err(base_field());
    }
    reap(db, version.project_id).await?;
    let busy = lab_training_runs::Entity::find()
        .filter(lab_training_runs::Column::ProjectId.eq(version.project_id))
        .filter(lab_training_runs::Column::Status.is_in(["queued", "running"]))
        .one(db)
        .await
        .map_err(LabError::db)?;
    if busy.is_some() {
        return Err(LabError::new(
            StatusCode::CONFLICT,
            err_codes::LAB_TRAIN_BUSY,
            "A training of this project is already queued or running.",
        ));
    }
    let row = lab_training_runs::ActiveModel {
        id: Set(Uuid::new_v4()),
        org_id: Set(org_id),
        project_id: Set(version.project_id),
        dataset_version_id: Set(version.id),
        base_model_id: Set(Some(base.id)),
        status: Set("queued".into()),
        epochs: Set(req.epochs as i32),
        progress: Set(serde_json::json!({})),
        map50: Set(None),
        error: Set(None),
        error_detail: Set(None),
        model_id: Set(None),
        created_by: Set(user.to_string()),
        created_at: Set(now()),
        heartbeat_at: Set(now()),
        finished_at: Set(None),
    }
    .insert(db)
    .await
    .map_err(LabError::db)?;
    tokio::spawn(run(ctx.clone(), row.id));
    Ok(view(&row, version.version, base.name))
}

/// Cancels a queued or running run (no-op on a finished one).
pub async fn cancel(db: &DatabaseConnection, org_id: i64, id: Uuid) -> Res<LabTrainingRun> {
    let run = lab_training_runs::Entity::find_by_id(id)
        .filter(lab_training_runs::Column::OrgId.eq(org_id))
        .one(db)
        .await
        .map_err(LabError::db)?
        .ok_or_else(|| not_found(err_codes::LAB_RUN_NOT_FOUND, "Training run"))?;
    let run = if LabRunStatus::from_wire(&run.status).active() {
        let mut a: lab_training_runs::ActiveModel = run.into();
        a.status = Set("cancelled".into());
        a.finished_at = Set(Some(now()));
        a.update(db).await.map_err(LabError::db)?
    } else {
        run
    };
    Ok(views(db, vec![run]).await?.remove(0))
}

/// Terminal write of a run, unless it was cancelled meanwhile.
async fn finish(
    db: &DatabaseConnection,
    id: Uuid,
    edit: impl FnOnce(&mut lab_training_runs::ActiveModel),
) {
    let Ok(Some(run)) = lab_training_runs::Entity::find_by_id(id).one(db).await else {
        return;
    };
    if run.status == "cancelled" {
        return;
    }
    let mut a: lab_training_runs::ActiveModel = run.into();
    a.finished_at = Set(Some(now()));
    edit(&mut a);
    if let Err(e) = a.update(db).await {
        tracing::error!(error = %e, run = %id, "lab training: final state not written");
    }
}

fn failed(
    code: &'static str,
    detail: Option<String>,
) -> impl FnOnce(&mut lab_training_runs::ActiveModel) {
    move |a| {
        a.status = Set("failed".into());
        a.error = Set(Some(code.to_string()));
        a.error_detail = Set(detail);
    }
}

/// The runner of one run: waits for the gate, trains, registers the model.
async fn run(ctx: loco_rs::app::AppContext, id: Uuid) {
    let db = ctx.db.clone();
    let beat = {
        let db = db.clone();
        tokio::spawn(async move {
            loop {
                let _ = lab_training_runs::Entity::update_many()
                    .col_expr(lab_training_runs::Column::HeartbeatAt, Expr::value(now()))
                    .filter(lab_training_runs::Column::Id.eq(id))
                    .exec(&db)
                    .await;
                tokio::time::sleep(HEARTBEAT).await;
            }
        })
    };
    let outcome = {
        let _permit = GATE.acquire().await;
        execute(&ctx, id).await
    };
    beat.abort();
    match outcome {
        Ok(()) => {}
        Err((code, detail)) => {
            tracing::warn!(run = %id, code, detail = ?detail, "lab training failed");
            finish(&db, id, failed(code, detail)).await;
        }
    }
}

type RunErr = (&'static str, Option<String>);

async fn execute(ctx: &loco_rs::app::AppContext, id: Uuid) -> Result<(), RunErr> {
    let db = &ctx.db;
    let internal = |e: String| (err_codes::LAB_TRAIN_FAILED, Some(e));
    let Some(run) = lab_training_runs::Entity::find_by_id(id)
        .one(db)
        .await
        .map_err(|e| internal(e.to_string()))?
    else {
        return Ok(());
    };
    if run.status != "queued" {
        return Ok(());
    }
    let mut a: lab_training_runs::ActiveModel = run.clone().into();
    a.status = Set("running".into());
    a.update(db).await.map_err(|e| internal(e.to_string()))?;

    let version = lab_dataset_versions::Entity::find_by_id(run.dataset_version_id)
        .one(db)
        .await
        .map_err(|e| internal(e.to_string()))?
        .ok_or((err_codes::LAB_VERSION_NOT_FOUND, None))?;
    let snap: DatasetSnapshot =
        serde_json::from_value(version.snapshot.clone()).map_err(|e| internal(e.to_string()))?;
    if snap.images.len() > EXPORT_MAX_IMAGES {
        return Err((
            err_codes::LAB_TRAIN_FAILED,
            Some(format!("more than {EXPORT_MAX_IMAGES} pictures")),
        ));
    }
    let base_invalid = |d: String| (err_codes::LAB_TRAIN_BASE_INVALID, Some(d));
    let base = match run.base_model_id {
        Some(bid) => ml_models::Entity::find_by_id(bid)
            .one(db)
            .await
            .map_err(|e| internal(e.to_string()))?,
        None => None,
    }
    .ok_or_else(|| base_invalid("the base model was deleted".into()))?;
    let (base_bytes, _) = vision::model_bytes(ctx, &base)
        .await
        .map_err(|e| internal(e.to_string()))?
        .ok_or_else(|| base_invalid("the base model file is missing".into()))?;

    // Pictures at their frozen versions (D199).
    let settings = MediaSettings::from_config(&ctx.config);
    let store = settings.store().map_err(internal)?;
    let mut images = Vec::with_capacity(snap.images.len());
    for img in &snap.images {
        let Ok(asset) = Uuid::parse_str(&img.asset_id) else {
            continue;
        };
        let Ok(Some(mv)) = media_versions::Entity::find()
            .filter(media_versions::Column::AssetId.eq(asset))
            .filter(media_versions::Column::OrgId.eq(run.org_id))
            .filter(media_versions::Column::VersionNumber.eq(img.asset_version))
            .one(db)
            .await
        else {
            continue;
        };
        let Ok(bytes) = store.get(&mv.storage_key).await else {
            continue;
        };
        images.push(TrainImage {
            bytes,
            boxes: img.boxes.iter().map(|b| (b.class, b.bbox)).collect(),
            val: img.split == LabSplit::Val,
        });
    }
    let classes = snap.classes.len();
    let epochs = run.epochs.max(1) as u32;

    let cancel = Arc::new(AtomicBool::new(false));
    let (tx, mut rx) = tokio::sync::watch::channel(LabRunProgress::default());
    let job = {
        let cancel = cancel.clone();
        tokio::task::spawn_blocking(move || {
            pnex_vision::train::train(&base_bytes, classes, epochs, &images, &mut |p| {
                let progress = match p {
                    Progress::Features { done, total } => LabRunProgress {
                        phase: "features".into(),
                        done: done as u32,
                        total: total as u32,
                        loss: None,
                    },
                    Progress::Epoch { epoch, total, loss } => LabRunProgress {
                        phase: "epochs".into(),
                        done: epoch,
                        total,
                        loss: Some(loss),
                    },
                    Progress::Evaluating => LabRunProgress {
                        phase: "evaluating".into(),
                        ..Default::default()
                    },
                };
                let _ = tx.send(progress);
                !cancel.load(Ordering::Relaxed)
            })
        })
    };
    tokio::pin!(job);
    let result = loop {
        tokio::select! {
            r = &mut job => break r,
            _ = tokio::time::sleep(TICK) => {
                let p = rx.borrow_and_update().clone();
                let _ = lab_training_runs::Entity::update_many()
                    .col_expr(lab_training_runs::Column::Progress, Expr::value(serde_json::to_value(&p).unwrap_or_default()))
                    .filter(lab_training_runs::Column::Id.eq(id))
                    .exec(db)
                    .await;
                let status = lab_training_runs::Entity::find_by_id(id)
                    .one(db)
                    .await
                    .ok()
                    .flatten()
                    .map(|r| r.status);
                if status.as_deref() != Some("running") {
                    cancel.store(true, Ordering::Relaxed);
                }
            }
        }
    };
    let outcome = match result {
        Ok(Ok(o)) => o,
        Ok(Err(TrainError::Cancelled)) => return Ok(()),
        Ok(Err(TrainError::NoData)) => return Err((err_codes::LAB_TRAIN_NO_DATA, None)),
        Ok(Err(e @ (TrainError::Head(_) | TrainError::DynamicInput))) => {
            return Err(base_invalid(e.to_string()))
        }
        Ok(Err(e)) => return Err(internal(e.to_string())),
        Err(e) => return Err(internal(e.to_string())),
    };

    // The ONNX becomes a model of the library and of the registry (D100).
    let project = find_project(db, run.org_id, run.project_id)
        .await
        .map_err(|e| internal(e.message))?;
    let name = format!(
        "{} · v{} · {}",
        project.name,
        version.version,
        &run.id.simple().to_string()[..8]
    );
    let spec = ModelSpec {
        family: VisionFamily::Yolox,
        labels: snap.classes.clone(),
        ..Default::default()
    };
    let (spec, infer_ms) = vision::prepare_spec(outcome.onnx.clone(), spec)
        .await
        .map_err(|e| internal(format!("{e:?}")))?;
    let asset = media_assets::ActiveModel {
        id: Set(Uuid::new_v4()),
        org_id: Set(run.org_id),
        kind: Set("model".into()),
        name: Set(name.clone()),
        description: Set(Some(format!(
            "Trained in the Vision Lab from {} (dataset v{}).",
            base.name, version.version
        ))),
        metadata: Set(None),
        ..Default::default()
    }
    .insert(db)
    .await
    .map_err(|e| internal(e.to_string()))?;
    crate::controllers::media::write_version(
        ctx,
        run.org_id,
        &asset,
        crate::controllers::media::IncomingVersion {
            filename: format!("lab-{}.onnx", run.id.simple()),
            content_type: "application/octet-stream".into(),
            note: None,
            bytes: axum::body::Bytes::from(outcome.onnx),
        },
        &settings,
    )
    .await
    .map_err(|e| internal(e.to_string()))?;
    let model = vision::insert_model(
        db,
        run.org_id,
        &name,
        Some(format!("Vision Lab, dataset v{}", version.version)),
        asset.id,
        None,
        &spec,
        infer_ms,
    )
    .await
    .map_err(|e| internal(e.to_string()))?;
    let map50 = outcome.map50;
    finish(db, id, move |a| {
        a.status = Set("succeeded".into());
        a.map50 = Set(map50);
        a.model_id = Set(Some(model.id));
        a.progress = Set(serde_json::json!({"phase": "done"}));
    })
    .await;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stale_window_is_longer_than_the_heartbeat() {
        assert!(STALE_SECS > 3 * HEARTBEAT.as_secs() as i64);
    }
}
