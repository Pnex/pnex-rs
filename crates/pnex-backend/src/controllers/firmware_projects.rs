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

/// 400 shapes (all `ErrorDetail`): field token (`field-invalid`), sketch
/// violations (`firmware-source-refused`), library code (`errors.args`).
fn write_error_response(e: FirmwareWriteError) -> Response {
    match e {
        FirmwareWriteError::Field(field, token) => super::field_error(field, &token),
        FirmwareWriteError::Source(violations) => {
            super::violations_error(pnex_core::err_codes::FIRMWARE_SOURCE_REFUSED, violations)
        }
        FirmwareWriteError::Lib(code, id) => super::coded_error(
            StatusCode::BAD_REQUEST,
            code,
            "Library not available for this project.",
            Some(serde_json::json!({ "value": id })),
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

/// Answer of `POST /api/v1/firmware-projects/{id}/check`.
#[derive(serde::Serialize, utoipa::ToSchema)]
struct CheckQueued {
    #[schema(value_type = String, format = "uuid")]
    check_id: uuid::Uuid,
    /// Always `queued`.
    status: pnex_core::firmware::FirmwareCheckState,
}

#[derive(serde::Deserialize, utoipa::IntoParams)]
#[into_params(parameter_in = Query)]
struct ListQuery {
    /// Label filter `name` / `name:value` (D218).
    label: Option<String>,
    #[param(value_type = Option<i64>)]
    limit: Option<String>,
    #[param(value_type = Option<i64>)]
    offset: Option<String>,
}

/// `limit`/`offset` of the paginated sub-collections.
#[derive(serde::Deserialize, utoipa::IntoParams)]
#[into_params(parameter_in = Query)]
struct PageQuery {
    #[param(value_type = Option<i64>)]
    limit: Option<String>,
    #[param(value_type = Option<i64>)]
    offset: Option<String>,
}

/// `GET /api/v1/firmware-projects` — org projects (name ASC).
#[utoipa::path(
    get,
    path = "/api/v1/firmware-projects",
    tag = "firmware-projects",
    operation_id = "firmware_projects_list",
    summary = "List the custom firmware projects",
    description = "Custom firmware projects of the org, ordered by name, with current revision number and attached device count, paginated (`limit`/`offset`). Optional label filter. Any org member.",
    params(ListQuery),
    responses(
        (status = 200, body = pnex_core::Paginated<FirmwareProjectSummary>),
        (status = 400, description = "Invalid `label` filter (`field-invalid`)", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn list(
    State(ctx): State<AppContext>,
    org: OrgContext,
    axum::extract::Query(q): axum::extract::Query<ListQuery>,
) -> Result<Response> {
    let mut select =
        firmware_projects::Entity::find().filter(firmware_projects::Column::OrgId.eq(org.org.id));
    match super::pagination::label_ids(
        &ctx.db,
        org.org.id,
        q.label.as_deref(),
        pnex_core::resources::KIND_FIRMWARE_PROJECT,
    )
    .await
    {
        Ok(None) => {}
        Ok(Some(ids)) => {
            select = select.filter(
                firmware_projects::Column::Id.is_in(super::pagination::parse_ids::<i64>(&ids)),
            )
        }
        Err(resp) => return Ok(resp),
    }
    let page = super::pagination::PageParams::from(q.limit.as_deref(), q.offset.as_deref());
    let (count, rows) = super::pagination::sql_page(
        &ctx.db,
        select.order_by_asc(firmware_projects::Column::Name),
        page,
    )
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
    let filters: Vec<(String, String)> = q
        .label
        .iter()
        .filter(|l| !l.is_empty())
        .map(|l| ("label".to_string(), l.clone()))
        .collect();
    format::json(super::pagination::envelope(
        "/api/v1/firmware-projects",
        &filters,
        page,
        count,
        results,
    ))
}

/// `GET /api/v1/firmware-projects/lib-catalog` — pinned library catalog.
#[utoipa::path(
    get,
    path = "/api/v1/firmware-projects/lib-catalog",
    tag = "firmware-projects",
    operation_id = "firmware_projects_lib_catalog",
    summary = "Pinned library catalog",
    description = "Libraries a custom firmware project may depend on (pinned PlatformIO specs, compatible chip families), paginated (`limit`/`offset`). Any org member.",
    params(PageQuery),
    responses(
        (status = 200, body = pnex_core::Paginated<LibCatalogItem>)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn lib_catalog(
    _org: OrgContext,
    axum::extract::Query(q): axum::extract::Query<PageQuery>,
) -> Result<Response> {
    let page = super::pagination::PageParams::from(q.limit.as_deref(), q.offset.as_deref());
    let (skip, take) = page.slice(LIB_CATALOG.len());
    let items: Vec<LibCatalogItem> = LIB_CATALOG
        .iter()
        .skip(skip)
        .take(take)
        .map(LibCatalogItem::from)
        .collect();
    format::json(super::pagination::envelope(
        "/api/v1/firmware-projects/lib-catalog",
        &[],
        page,
        LIB_CATALOG.len() as i64,
        items,
    ))
}

/// `POST /api/v1/firmware-projects` — project + revision 1 (201).
#[utoipa::path(
    post,
    path = "/api/v1/firmware-projects",
    tag = "firmware-projects",
    operation_id = "firmware_projects_create",
    summary = "Create a custom firmware project",
    description = "Creates the project and its revision 1 (`main_cpp` absent = starter sketch). The sketch passes a lexical policy guard and the libraries must suit the chip family. Owner, admin or member.",
    request_body = pnex_core::firmware::CreateFirmwareProject,
    responses(
        (status = 201, body = pnex_core::firmware::FirmwareProjectDetail),
        (status = 400, description = "Invalid input: a field token (`field-invalid`, `errors.fields`), refused sketch constructs (`firmware-source-refused`, `errors.violations`: [SourceViolation]), or an unavailable library (`firmware-lib-*`, `errors.args.value`)", body = crate::openapi::ErrorDetail),
        (status = 403, description = "Viewer (`firmware-write-forbidden`)", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
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
#[utoipa::path(
    get,
    path = "/api/v1/firmware-projects/{id}",
    tag = "firmware-projects",
    operation_id = "firmware_projects_get",
    summary = "Get a firmware project with its current sketch",
    description = "Project metadata plus the current revision content (sketch and libraries). Any org member.",
    params(("id" = i64, Path, description = "Firmware project id")),
    responses(
        (status = 200, body = pnex_core::firmware::FirmwareProjectDetail),
        (status = 404, description = "Unknown project (or other org)", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
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
#[utoipa::path(
    patch,
    path = "/api/v1/firmware-projects/{id}",
    tag = "firmware-projects",
    operation_id = "firmware_projects_save",
    summary = "Save a new revision and/or metadata",
    description = "Appends a revision (sketch/libraries) and/or updates name/description. Optimistic concurrency: `expected_revision_number` must equal the current revision. A save with identical content creates no revision. Owner, admin or member.",
    params(("id" = i64, Path, description = "Firmware project id")),
    request_body = pnex_core::firmware::SaveFirmwareRevision,
    responses(
        (status = 200, body = pnex_core::firmware::FirmwareProjectDetail),
        (status = 400, description = "Invalid input: a field token (`field-invalid`, `errors.fields`), refused sketch constructs (`firmware-source-refused`, `errors.violations`: [SourceViolation]), or an unavailable library (`firmware-lib-*`, `errors.args.value`)", body = crate::openapi::ErrorDetail),
        (status = 403, description = "Viewer (`firmware-write-forbidden`)", body = crate::openapi::ErrorDetail),
        (status = 404, description = "Unknown project (or other org)", body = crate::openapi::ErrorDetail),
        (status = 409, description = "Stale `expected_revision_number` (`firmware-version-conflict`)", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
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
        return Err(error(
            StatusCode::CONFLICT,
            "firmware-version-conflict",
            "Someone saved a newer revision first — the editor was reloaded.",
        ));
    }
    match save_revision(&ctx.db, p, rev, &input).await {
        Ok((p, rev)) => format::json(detail_dto(&p, &rev)),
        Err(e) => Ok(write_error_response(e)),
    }
}

/// `GET /api/v1/firmware-projects/{id}/revisions` — history (DESC).
#[utoipa::path(
    get,
    path = "/api/v1/firmware-projects/{id}/revisions",
    tag = "firmware-projects",
    operation_id = "firmware_projects_revisions",
    summary = "Revision history of a firmware project",
    description = "Every revision, newest first, with full content (any one can be restored by saving it), paginated (`limit`/`offset`). Any org member.",
    params(("id" = i64, Path, description = "Firmware project id"), PageQuery),
    responses(
        (status = 200, body = pnex_core::Paginated<FirmwareRevisionSummary>),
        (status = 404, description = "Unknown project (or other org)", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn revisions(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(id): Path<i64>,
    axum::extract::Query(q): axum::extract::Query<PageQuery>,
) -> Result<Response> {
    let (p, _) = find_project(&ctx.db, &org, id).await?;
    let page = super::pagination::PageParams::from(q.limit.as_deref(), q.offset.as_deref());
    let (count, rows) = super::pagination::sql_page(
        &ctx.db,
        firmware_revisions::Entity::find()
            .filter(firmware_revisions::Column::FirmwareProjectId.eq(p.id))
            .order_by_desc(firmware_revisions::Column::RevisionNumber),
        page,
    )
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
    format::json(super::pagination::envelope(
        &format!("/api/v1/firmware-projects/{id}/revisions"),
        &[],
        page,
        count,
        results,
    ))
}

/// `DELETE /api/v1/firmware-projects/{id}` — 409 while devices are
/// attached; 204 otherwise (revisions cascade).
#[utoipa::path(
    delete,
    path = "/api/v1/firmware-projects/{id}",
    tag = "firmware-projects",
    operation_id = "firmware_projects_delete",
    summary = "Delete a firmware project",
    description = "Deletes the project and all its revisions. Refused while devices use it. Owner, admin or member.",
    params(("id" = i64, Path, description = "Firmware project id")),
    responses(
        (status = 204, description = "Deleted"),
        (status = 403, description = "Viewer (`firmware-write-forbidden`)", body = crate::openapi::ErrorDetail),
        (status = 404, description = "Unknown project (or other org)", body = crate::openapi::ErrorDetail),
        (status = 409, description = "Devices attached (`firmware-project-in-use`, args.count)", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
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
        return Err(super::coded_error(
            StatusCode::CONFLICT,
            pnex_core::err_codes::FIRMWARE_PROJECT_IN_USE,
            "Devices use this firmware project.",
            Some(serde_json::json!({ "count": n.to_string() })),
        ));
    }
    // Revisions go with the project (FK cascade on firmware_project_id).
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
#[utoipa::path(
    post,
    path = "/api/v1/firmware-projects/{id}/check",
    tag = "firmware-projects",
    operation_id = "firmware_projects_check",
    summary = "Start a compile-only check",
    description = "Queues a background compile of the current revision (no artifact produced); poll `GET /api/v1/firmware-projects/{id}/checks/{check_id}`. Requires custom firmware builds enabled on the server. Any org member, viewers included.",
    params(("id" = i64, Path, description = "Firmware project id")),
    responses(
        (status = 202, body = CheckQueued),
        (status = 400, description = "Library not available for the chip (`firmware-lib-*`)", body = crate::openapi::ErrorDetail),
        (status = 404, description = "Unknown project (or other org)", body = crate::openapi::ErrorDetail),
        (status = 409, description = "Custom builds disabled (`firmware-custom-disabled`)", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
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
        format::json(CheckQueued {
            check_id: check.id,
            status: pnex_core::firmware::FirmwareCheckState::Queued,
        }),
    )
        .into_response())
}

/// `GET /api/v1/firmware-projects/{id}/checks/{check_id}` — check state.
#[utoipa::path(
    get,
    path = "/api/v1/firmware-projects/{id}/checks/{check_id}",
    tag = "firmware-projects",
    operation_id = "firmware_projects_check_status",
    summary = "State of a compile-only check",
    description = "Status (`running` | `succeeded` | `failed` | `error`), compiler diagnostics and the output tail. Any org member.",
    params(("id" = i64, Path, description = "Firmware project id"), ("check_id" = String, Path, format = "uuid", description = "Check id")),
    responses(
        (status = 200, body = pnex_core::firmware::FirmwareCheckStatus),
        (status = 404, description = "Unknown project or check", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
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
        // Unknown stored states never happen; `error` is the safe reading.
        status: pnex_core::firmware::FirmwareCheckState::from_wire(&state.status)
            .unwrap_or(pnex_core::firmware::FirmwareCheckState::Error),
        revision_number: state.revision_number,
        diagnostics: state
            .diagnostics
            .and_then(|d| serde_json::from_value(d).ok())
            .unwrap_or_default(),
        log_tail: state.log_tail.unwrap_or_default(),
    })
}

/// OpenAPI paths of this controller (merged by `crate::openapi::spec`).
pub fn openapi() -> utoipa::openapi::OpenApi {
    #[derive(utoipa::OpenApi)]
    #[openapi(paths(
        list,
        lib_catalog,
        create,
        detail,
        save,
        revisions,
        delete,
        check,
        check_status
    ))]
    struct Doc;
    <Doc as utoipa::OpenApi>::openapi()
}
