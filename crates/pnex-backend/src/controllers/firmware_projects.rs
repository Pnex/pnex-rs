//! Custom firmware projects (custom-firmware.md D89–D92) — org-scoped CRUD,
//! append-only revisions, library catalog, compile-only « Verify » checks
//! (devices pick a project at provisioning — `CreateDevice.firmware_project_id`). School `controllers/functions.rs`: cross-org 404,
//! writes gated by `can_write()`, 400 field by field with machine tokens.
//!
//! Compiling (check or device build) requires `settings.firmware.custom.
//! enabled` (off by default, D91): editing and history work regardless.

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::Json;
use loco_rs::prelude::*;
use sea_orm::{ColumnTrait, EntityTrait, QueryFilter, QueryOrder};

use crate::auth::OrgContext;
use crate::models::_entities::{firmware_projects, firmware_revisions};
use crate::services::firmware::FirmwareSettings;
use crate::services::firmware_projects::{
    check_inputs, create_check, create_project, device_count, get_check, latest_revision,
    revision_libs, save_revision, FirmwareWriteError,
};
use crate::workers::firmware_check::{FirmwareCheckArgs, FirmwareCheckWorker};
use pnex_core::firmware::{
    CreateFirmwareProject, FirmwareProjectDetail, FirmwareProjectSummary, FirmwareRevisionSummary,
    LibCatalogItem, SaveFirmwareRevision, LIB_CATALOG,
};

pub fn routes() -> Routes {
    Routes::new()
        .prefix("/api/v1")
        .add("/firmware-projects", get(list).post(create))
        .add("/firmware-projects/lib-catalog", get(lib_catalog))
        .add(
            "/firmware-projects/{id}",
            get(detail).patch(save).delete(delete),
        )
        .add("/firmware-projects/{id}/revisions", get(revisions))
        .add("/firmware-projects/{id}/check", post(check))
        .add(
            "/firmware-projects/{id}/checks/{check_id}",
            get(check_status),
        )
}

fn error(status: StatusCode, code: &str, msg: &str) -> Error {
    Error::CustomError(
        status,
        loco_rs::controller::ErrorDetail::new(code, msg.to_string()),
    )
}

fn forbidden() -> Error {
    error(
        StatusCode::FORBIDDEN,
        "firmware-write-forbidden",
        "Owner, admin or member role required to manage firmware projects.",
    )
}

/// 400 shapes: field token (`{field: token}`), sketch violations
/// (`{error, violations}`), library code (`{error, errors.args}`).
fn write_error_response(e: FirmwareWriteError) -> Response {
    match e {
        FirmwareWriteError::Field(field, token) => (
            StatusCode::BAD_REQUEST,
            format::json(serde_json::json!({ field: token })),
        )
            .into_response(),
        FirmwareWriteError::Source(violations) => (
            StatusCode::BAD_REQUEST,
            format::json(serde_json::json!({
                "error": "firmware-source-refused",
                "description": "The sketch contains constructs refused by the build policy.",
                "violations": violations,
            })),
        )
            .into_response(),
        FirmwareWriteError::Lib(code, id) => (
            StatusCode::BAD_REQUEST,
            format::json(serde_json::json!({
                "error": code,
                "description": "Library not available for this project.",
                "errors": { "args": { "value": id } },
            })),
        )
            .into_response(),
        FirmwareWriteError::Db(msg) => {
            tracing::error!("firmware project write: {msg}");
            Error::InternalServerError.into_response()
        }
    }
}

fn summary_dto(p: &firmware_projects::Model, current: i64, devices: i64) -> FirmwareProjectSummary {
    FirmwareProjectSummary {
        id: p.id,
        name: p.name.clone(),
        description: p.description.clone(),
        chip_family: p.chip_family.clone(),
        current_revision_number: current,
        device_count: devices,
        created_at: p.created_at.to_rfc3339(),
        updated_at: p.updated_at.to_rfc3339(),
    }
}

fn detail_dto(
    p: &firmware_projects::Model,
    rev: &firmware_revisions::Model,
) -> FirmwareProjectDetail {
    FirmwareProjectDetail {
        id: p.id,
        name: p.name.clone(),
        description: p.description.clone(),
        chip_family: p.chip_family.clone(),
        current_revision_number: rev.revision_number,
        main_cpp: rev.main_cpp.clone(),
        lib_deps: revision_libs(rev),
        created_at: p.created_at.to_rfc3339(),
        updated_at: p.updated_at.to_rfc3339(),
    }
}

async fn find_project(
    db: &DatabaseConnection,
    org: &OrgContext,
    id: i64,
) -> Result<(firmware_projects::Model, firmware_revisions::Model)> {
    let project = firmware_projects::Entity::find_by_id(id)
        .filter(firmware_projects::Column::OrgId.eq(org.org.id))
        .one(db)
        .await
        .map_err(|_| Error::InternalServerError)?
        .ok_or(Error::NotFound)?;
    let rev = latest_revision(db, project.id)
        .await
        .map_err(|_| Error::InternalServerError)?
        .ok_or(Error::NotFound)?;
    Ok((project, rev))
}

/// `GET /api/v1/firmware-projects` — org projects (name ASC).
async fn list(State(ctx): State<AppContext>, org: OrgContext) -> Result<Response> {
    let rows = firmware_projects::Entity::find()
        .filter(firmware_projects::Column::OrgId.eq(org.org.id))
        .order_by_asc(firmware_projects::Column::Name)
        .all(&ctx.db)
        .await
        .map_err(|_| Error::InternalServerError)?;
    let mut results = Vec::with_capacity(rows.len());
    for p in &rows {
        let current = latest_revision(&ctx.db, p.id)
            .await
            .map_err(|_| Error::InternalServerError)?
            .map(|r| r.revision_number)
            .unwrap_or(0);
        let devices = device_count(&ctx.db, p.id)
            .await
            .map_err(|_| Error::InternalServerError)?;
        results.push(summary_dto(p, current, devices));
    }
    format::json(serde_json::json!({ "count": results.len(), "results": results }))
}

/// `GET /api/v1/firmware-projects/lib-catalog` — pinned library catalog.
async fn lib_catalog(_org: OrgContext) -> Result<Response> {
    let items: Vec<LibCatalogItem> = LIB_CATALOG.iter().map(LibCatalogItem::from).collect();
    format::json(serde_json::json!({ "count": items.len(), "results": items }))
}

/// `POST /api/v1/firmware-projects` — project + revision 1 (201).
async fn create(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Json(input): Json<CreateFirmwareProject>,
) -> Result<Response> {
    if !org.can_write() {
        return Err(forbidden());
    }
    match create_project(&ctx.db, org.org.id, &input).await {
        Ok((p, rev)) => {
            Ok((StatusCode::CREATED, format::json(detail_dto(&p, &rev))).into_response())
        }
        Err(e) => Ok(write_error_response(e)),
    }
}

/// `GET /api/v1/firmware-projects/{id}` — project + current revision.
async fn detail(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(id): Path<i64>,
) -> Result<Response> {
    let (p, rev) = find_project(&ctx.db, &org, id).await?;
    format::json(detail_dto(&p, &rev))
}

/// `PATCH /api/v1/firmware-projects/{id}` — new revision and/or metadata;
/// 409 `version_conflict` when `expected_revision_number` is stale.
async fn save(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(id): Path<i64>,
    Json(input): Json<SaveFirmwareRevision>,
) -> Result<Response> {
    if !org.can_write() {
        return Err(forbidden());
    }
    let (p, rev) = find_project(&ctx.db, &org, id).await?;
    if input.expected_revision_number != rev.revision_number {
        return Ok((
            StatusCode::CONFLICT,
            format::json(serde_json::json!({
                "error": "firmware-version-conflict",
                "description": "Someone saved a newer revision first — the editor was reloaded.",
            })),
        )
            .into_response());
    }
    match save_revision(&ctx.db, p, rev, &input).await {
        Ok((p, rev)) => format::json(detail_dto(&p, &rev)),
        Err(e) => Ok(write_error_response(e)),
    }
}

/// `GET /api/v1/firmware-projects/{id}/revisions` — history (DESC).
async fn revisions(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(id): Path<i64>,
) -> Result<Response> {
    let (p, _) = find_project(&ctx.db, &org, id).await?;
    let rows = firmware_revisions::Entity::find()
        .filter(firmware_revisions::Column::FirmwareProjectId.eq(p.id))
        .order_by_desc(firmware_revisions::Column::RevisionNumber)
        .all(&ctx.db)
        .await
        .map_err(|_| Error::InternalServerError)?;
    let results: Vec<FirmwareRevisionSummary> = rows
        .iter()
        .map(|r| FirmwareRevisionSummary {
            id: r.id,
            revision_number: r.revision_number,
            note: r.note.clone(),
            main_cpp: r.main_cpp.clone(),
            lib_deps: revision_libs(r),
            created_at: r.created_at.to_rfc3339(),
        })
        .collect();
    format::json(serde_json::json!({ "count": results.len(), "results": results }))
}

/// `DELETE /api/v1/firmware-projects/{id}` — 409 while devices are
/// attached; 204 otherwise (revisions cascade).
async fn delete(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(id): Path<i64>,
) -> Result<Response> {
    if !org.can_write() {
        return Err(forbidden());
    }
    let (p, _) = find_project(&ctx.db, &org, id).await?;
    let n = device_count(&ctx.db, p.id)
        .await
        .map_err(|_| Error::InternalServerError)?;
    if n > 0 {
        return Ok((
            StatusCode::CONFLICT,
            format::json(serde_json::json!({
                "error": "firmware-project-in-use",
                "description": "Devices use this firmware project.",
                "errors": { "args": { "count": n.to_string() } },
            })),
        )
            .into_response());
    }
    // sqlite: no FK on current_revision_id — clear the pointer, then the
    // revisions go with the project (FK cascade on firmware_project_id).
    firmware_revisions::Entity::delete_many()
        .filter(firmware_revisions::Column::FirmwareProjectId.eq(p.id))
        .exec(&ctx.db)
        .await
        .map_err(|_| Error::InternalServerError)?;
    firmware_projects::Entity::delete_by_id(p.id)
        .exec(&ctx.db)
        .await
        .map_err(|_| Error::InternalServerError)?;
    Ok(StatusCode::NO_CONTENT.into_response())
}

/// Custom compile gate (D91) — 409 `firmware-custom-disabled` when off.
fn custom_settings(ctx: &AppContext) -> Result<FirmwareSettings> {
    let settings = FirmwareSettings::from_config(&ctx.config);
    if !settings.custom.enabled {
        return Err(error(
            StatusCode::CONFLICT,
            "firmware-custom-disabled",
            "Custom firmware builds are disabled on this server.",
        ));
    }
    if settings.custom.sandbox.is_none() {
        tracing::warn!(
            "custom firmware compiled WITHOUT sandbox (settings.firmware.custom.sandbox = none)"
        );
    }
    Ok(settings)
}

/// `POST /api/v1/firmware-projects/{id}/check` — compile-only check of the
/// current revision (202 + `check_id`; poll `checks/{check_id}`). Viewer
/// allowed (read-only tool, no artifact).
async fn check(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(id): Path<i64>,
) -> Result<Response> {
    custom_settings(&ctx)?;
    let (p, rev) = find_project(&ctx.db, &org, id).await?;
    // Fail fast on a library the chip no longer accepts (same message as
    // the save guard) — the worker would only report it later.
    if let Err(msg) = check_inputs(&p, &rev, None) {
        let code = match msg.split(':').next() {
            Some(c) if c.starts_with("firmware-lib-") => c.to_string(),
            _ => "firmware-lib-unknown".to_string(),
        };
        return Err(error(StatusCode::BAD_REQUEST, &code, &msg));
    }
    let check = create_check(&ctx.db, &p, rev.revision_number)
        .await
        .map_err(|_| Error::InternalServerError)?;
    FirmwareCheckWorker::perform_later(&ctx, FirmwareCheckArgs { check_id: check.id })
        .await
        .map_err(|_| Error::InternalServerError)?;
    Ok((
        StatusCode::ACCEPTED,
        format::json(serde_json::json!({ "check_id": check.id, "status": "queued" })),
    )
        .into_response())
}

/// `GET /api/v1/firmware-projects/{id}/checks/{check_id}` — check state.
async fn check_status(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path((id, check_id)): Path<(i64, uuid::Uuid)>,
) -> Result<Response> {
    let (p, _) = find_project(&ctx.db, &org, id).await?;
    let state = get_check(&ctx.db, org.org.id, p.id, check_id)
        .await
        .map_err(|_| Error::InternalServerError)?
        .ok_or(Error::NotFound)?;
    format::json(pnex_core::firmware::FirmwareCheckStatus {
        check_id: state.id.to_string(),
        status: state.status,
        revision_number: state.revision_number,
        diagnostics: state
            .diagnostics
            .and_then(|d| serde_json::from_value(d).ok())
            .unwrap_or_default(),
        log_tail: state.log_tail.unwrap_or_default(),
    })
}
