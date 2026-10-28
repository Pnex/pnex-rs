//! "Functions" registry — org-scoped CRUD + append-only versioning +
//! live test. `controllers/edge_refs.rs` school: org scoping (cross-org 404
//! masked), writes gated by their IAM permission, 400 field by field. The test
//! spawns the runtime binary (`--test-function`) — viewer allowed (read).

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::routing::get;
use axum::Json;
use loco_rs::prelude::*;
use sea_orm::{ColumnTrait, EntityTrait, QueryFilter, QueryOrder};
use serde::Deserialize;

use crate::auth::OrgContext;
use crate::controllers::pagination;
use crate::models::_entities::{function_versions, functions};
use crate::services::flow::FlowSettings;
use crate::services::functions::{
    create_function, latest_version, referencing_deployed_flows, save_new_version,
    spawn_function_check, spawn_function_test, FunctionWriteError,
};
use pnex_core::{
    CreateFunction, FunctionDetail, FunctionSummary, FunctionTestRequest, FunctionValidateRequest,
    FunctionVersionSummary, SaveFunctionVersion,
};

// Writes: owner/admin/member (`OrgContext::can_write`).
// Error helpers carry a machine code (`pnex_core::err_codes` registry) so
// the frontend can resolve `err-<kebab>` at render time; the English text is
// the verbatim fallback for unregistered codes.

/// 503 `functions-runtime-disabled`: the flow runtime is not enabled.
fn runtime_disabled() -> Error {
    Error::CustomError(
        StatusCode::SERVICE_UNAVAILABLE,
        loco_rs::controller::ErrorDetail::new(
            "functions-runtime-disabled",
            "The flow runtime is not enabled (settings.flow.enabled)",
        ),
    )
}

/// 503 `flow-runtime`: infrastructure failure of a live test/check
/// (binary missing, timeout, unreadable output); the diagnostic travels
/// verbatim in `args.detail` (runtime diagnostics exception).
fn flow_runtime_503(detail: &str) -> Error {
    super::coded_error(
        StatusCode::SERVICE_UNAVAILABLE,
        pnex_core::err_codes::FLOW_RUNTIME,
        "The flow runtime is unavailable.",
        Some(serde_json::json!({ "detail": detail })),
    )
}

pub fn routes() -> Routes {
    Routes::new()
        .prefix("/api/v1/functions")
        .add("", get(list).post(create))
        .add("/{id}", get(detail).patch(save_version).delete(delete))
        .add("/{id}/versions", get(versions))
        .add("/{id}/test", post(test))
        .add("/validate", post(validate))
}

/// OpenAPI paths of this controller (merged by `crate::openapi::spec`).
pub fn openapi() -> utoipa::openapi::OpenApi {
    #[derive(utoipa::OpenApi)]
    #[openapi(paths(list, create, detail, save_version, versions, test, validate, delete))]
    struct Doc;
    <Doc as utoipa::OpenApi>::openapi()
}

// ───────────────────────────── DTO mappers ─────────────────────────────

/// FunctionDetail — save_version returns the fresh version (the editor
/// reloads without a second GET).
fn detail_dto(f: &functions::Model, version: &function_versions::Model) -> FunctionDetail {
    FunctionDetail {
        id: f.id,
        org_id: f.org_id,
        name: f.name.clone(),
        language: crate::services::functions::parse_language(&f.language).unwrap_or_default(),
        description: f.description.clone(),
        current_version_number: version.version_number,
        code: version.code.clone(),
        inputs: serde_json::from_value(version.inputs.clone()).unwrap_or_default(),
        outputs: serde_json::from_value(version.outputs.clone()).unwrap_or_default(),
        created_at: f.created_at.to_rfc3339(),
        updated_at: f.updated_at.to_rfc3339(),
    }
}

/// Summary (list, without code).
fn summary_dto(f: &functions::Model, current: i64) -> FunctionSummary {
    FunctionSummary {
        id: f.id,
        org_id: f.org_id,
        name: f.name.clone(),
        language: crate::services::functions::parse_language(&f.language).unwrap_or_default(),
        description: f.description.clone(),
        current_version_number: current,
        created_at: f.created_at.to_rfc3339(),
        updated_at: f.updated_at.to_rfc3339(),
    }
}

// ─────────────────────────────── Handlers ───────────────────────────────

#[derive(Deserialize, utoipa::IntoParams)]
#[into_params(parameter_in = Query)]
struct ListQuery {
    #[param(value_type = Option<i64>)]
    limit: Option<String>,
    #[param(value_type = Option<i64>)]
    offset: Option<String>,
    /// Label filter `name` / `name:value` (D218).
    label: Option<String>,
}

/// `limit`/`offset` of a paginated sub-collection.
#[derive(Deserialize, utoipa::IntoParams)]
#[into_params(parameter_in = Query)]
struct PageQuery {
    #[param(value_type = Option<i64>)]
    limit: Option<String>,
    #[param(value_type = Option<i64>)]
    offset: Option<String>,
}

/// `GET /api/v1/functions` — the org's registry (name ASC). The current
/// version number is computed in batch (one versions query for the batch).
#[utoipa::path(
    get,
    path = "/api/v1/functions",
    tag = "functions",
    operation_id = "functions_list",
    summary = "List the functions of the organisation",
    description = "Paginated registry of reusable JS/Starlark functions, sorted by name, without code. Each item carries its current version number.",
    params(ListQuery),
    responses(
        (status = 200, body = pnex_core::Paginated<FunctionSummary>),
        (status = 400, description = "`field-invalid`: invalid `label` filter", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn list(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Query(q): Query<ListQuery>,
) -> Result<Response> {
    let page = pagination::PageParams::from(q.limit.as_deref(), q.offset.as_deref());
    let mut select = functions::Entity::find().filter(functions::Column::OrgId.eq(org.org.id));
    match pagination::label_ids(
        &ctx.db,
        org.org.id,
        q.label.as_deref(),
        pnex_core::resources::KIND_FUNCTION,
    )
    .await
    {
        Ok(None) => {}
        Ok(Some(ids)) => {
            select = select.filter(functions::Column::Id.is_in(pagination::parse_ids::<i64>(&ids)))
        }
        Err(resp) => return Ok(resp),
    }
    let rows = select
        .order_by_asc(functions::Column::Name)
        .all(&ctx.db)
        .await
        .map_err(|_| Error::InternalServerError)?;
    let count = rows.len() as i64;
    // Current version numbers of the batch: a single versions query.
    let ids: Vec<i64> = rows.iter().map(|f| f.id).collect();
    let all_versions = if ids.is_empty() {
        Vec::new()
    } else {
        function_versions::Entity::find()
            .filter(function_versions::Column::FunctionId.is_in(ids))
            .all(&ctx.db)
            .await
            .map_err(|_| Error::InternalServerError)?
    };
    let page_slice: Vec<FunctionSummary> = rows
        .iter()
        .map(|f| {
            let current = all_versions
                .iter()
                .filter(|v| v.function_id == f.id)
                .map(|v| v.version_number)
                .max()
                .unwrap_or(0);
            summary_dto(f, current)
        })
        .skip(page.slice(count as usize).0)
        .take(page.slice(count as usize).1)
        .collect();
    let label_filter: Vec<(String, String)> = q
        .label
        .iter()
        .filter(|l| !l.is_empty())
        .map(|l| ("label".to_string(), l.clone()))
        .collect();
    Ok(format::json(pagination::envelope(
        "/api/v1/functions",
        &label_filter,
        page,
        count,
        page_slice,
    ))
    .into_response())
}

/// `POST /api/v1/functions` — function + version 1. 201 = FunctionDetail
/// (the editor chains without a second GET); 400 field by field.
#[utoipa::path(
    post,
    path = "/api/v1/functions",
    tag = "functions",
    operation_id = "functions_create",
    summary = "Create a function",
    description = "Creates a function and its version 1 from the code; the `@input`/`@output` directives of the code define its interface. Requires owner, admin or member role.",
    request_body = CreateFunction,
    responses(
        (status = 201, body = FunctionDetail),
        (status = 400, description = "`field-invalid` (`errors.fields`: `name`) or `function-directive-invalid` (`args.line`, `args.detail`)", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn create(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Json(input): Json<CreateFunction>,
) -> Result<Response> {
    let (f, version) = match create_function(&ctx.db, org.org.id, &input).await {
        Ok(pair) => pair,
        Err(e) => return Ok(write_error_response(e)),
    };
    Ok((StatusCode::CREATED, format::json(detail_dto(&f, &version))).into_response())
}

/// Write errors: `field-invalid` 400 (field tokens), 400
/// `function-directive-invalid` (`args.line`, `args.detail`), 409, 500.
fn write_error_response(e: FunctionWriteError) -> Response {
    match e {
        FunctionWriteError::Field(field, token) => super::field_error(&field, &token),
        FunctionWriteError::Directive(e) => super::coded_error(
            StatusCode::BAD_REQUEST,
            pnex_core::err_codes::FUNCTION_DIRECTIVE_INVALID,
            "An @input/@output directive of the code is invalid.",
            Some(serde_json::json!({ "line": e.line.to_string(), "detail": e.message })),
        )
        .into_response(),
        FunctionWriteError::Db(_) => Error::InternalServerError.into_response(),
        FunctionWriteError::Conflict { .. } => crate::controllers::coded_error(
            StatusCode::CONFLICT,
            pnex_core::err_codes::FUNCTION_VERSION_CONFLICT,
            "The function changed since it was read.",
            None,
        )
        .into_response(),
    }
}

/// Requested function of the org, otherwise 404 (cross-org masked).
async fn find_function(
    db: &DatabaseConnection,
    org: &OrgContext,
    id: i64,
) -> Result<Option<functions::Model>> {
    let row = functions::Entity::find_by_id(id)
        .filter(functions::Column::OrgId.eq(org.org.id))
        .one(db)
        .await
        .map_err(|_| Error::InternalServerError)?;
    Ok(row)
}

/// `GET /api/v1/functions/{id}` — function + current version (code +
/// extracted interface).
#[utoipa::path(
    get,
    path = "/api/v1/functions/{id}",
    tag = "functions",
    operation_id = "functions_get",
    summary = "Get a function",
    description = "The function with the code and interface of its current version. 404 when it does not exist or belongs to another organisation.",
    params(("id" = i64, Path, description = "Function id")),
    responses(
        (status = 200, body = FunctionDetail),
        (status = 404, body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn detail(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(id): Path<i64>,
) -> Result<Response> {
    let Some(f) = find_function(&ctx.db, &org, id).await? else {
        return Err(Error::NotFound);
    };
    let Some(version) = latest_version(&ctx.db, f.id)
        .await
        .map_err(|_| Error::InternalServerError)?
    else {
        return Err(Error::NotFound);
    };
    Ok(format::json(detail_dto(&f, &version)).into_response())
}

/// `PATCH /api/v1/functions/{id}` — new version (append-only) and/or
/// metadata. 409 if `expected_version_number` ≠ current version.
#[utoipa::path(
    patch,
    path = "/api/v1/functions/{id}",
    tag = "functions",
    operation_id = "functions_save_version",
    summary = "Save a new version of a function",
    description = "With `code`, appends a new version (history is append-only); `name`/`description` update metadata. Optimistic concurrency: `expected_version_number` must equal the current version, else 409 `function-version-conflict`. Deployed flows keep the version they pinned. Requires owner, admin or member role.",
    params(("id" = i64, Path, description = "Function id")),
    request_body = SaveFunctionVersion,
    responses(
        (status = 200, body = FunctionDetail),
        (status = 400, description = "`field-invalid` (`errors.fields`: `name`) or `function-directive-invalid` (`args.line`, `args.detail`)", body = crate::openapi::ErrorDetail),
        (status = 404, body = crate::openapi::ErrorDetail),
        (status = 409, description = "`function-version-conflict`", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn save_version(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(id): Path<i64>,
    Json(input): Json<SaveFunctionVersion>,
) -> Result<Response> {
    let Some(f) = find_function(&ctx.db, &org, id).await? else {
        return Err(Error::NotFound);
    };
    // Optimistic concurrency (flows school): the comparison is against the
    // highest saved version (append-only).
    let current = latest_version(&ctx.db, f.id)
        .await
        .map_err(|_| Error::InternalServerError)?
        .map(|v| v.version_number)
        .unwrap_or(0);
    if input.expected_version_number != current {
        return Err(crate::controllers::coded_error(
            StatusCode::CONFLICT,
            pnex_core::err_codes::FUNCTION_VERSION_CONFLICT,
            "The function changed since it was read.",
            None,
        ));
    }
    let (f, new_version) = match save_new_version(&ctx.db, f, &input).await {
        Ok(pair) => pair,
        Err(e) => return Ok(write_error_response(e)),
    };
    let version = match new_version {
        Some(v) => v,
        None => latest_version(&ctx.db, f.id)
            .await
            .map_err(|_| Error::InternalServerError)?
            .ok_or(Error::NotFound)?,
    };
    Ok(format::json(detail_dto(&f, &version)).into_response())
}

/// `GET /api/v1/functions/{id}/versions` — historique (version DESC).
#[utoipa::path(
    get,
    path = "/api/v1/functions/{id}/versions",
    tag = "functions",
    operation_id = "functions_versions",
    summary = "List the versions of a function",
    description = "Version history, newest first, each with its code and interface. Paginated (`limit`/`offset`).",
    params(("id" = i64, Path, description = "Function id"), PageQuery),
    responses(
        (status = 200, body = pnex_core::Paginated<FunctionVersionSummary>),
        (status = 404, body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn versions(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(id): Path<i64>,
    Query(q): Query<PageQuery>,
) -> Result<Response> {
    let Some(f) = find_function(&ctx.db, &org, id).await? else {
        return Err(Error::NotFound);
    };
    let page = pagination::PageParams::from(q.limit.as_deref(), q.offset.as_deref());
    let select = function_versions::Entity::find()
        .filter(function_versions::Column::FunctionId.eq(f.id))
        .order_by_desc(function_versions::Column::VersionNumber);
    let (count, rows) = pagination::sql_page(&ctx.db, select, page)
        .await
        .map_err(|_| Error::InternalServerError)?;
    let results: Vec<FunctionVersionSummary> = rows
        .iter()
        .map(|v| FunctionVersionSummary {
            id: v.id,
            version_number: v.version_number,
            note: v.note.clone(),
            inputs: serde_json::from_value(v.inputs.clone()).unwrap_or_default(),
            outputs: serde_json::from_value(v.outputs.clone()).unwrap_or_default(),
            created_at: v.created_at.to_rfc3339(),
            code: v.code.clone(),
        })
        .collect();
    Ok(format::json(pagination::envelope(
        &format!("/api/v1/functions/{id}/versions"),
        &[],
        page,
        count,
        results,
    ))
    .into_response())
}

/// `DELETE /api/v1/functions/{id}`: 409 `function-in-use` while a deployed
/// flow references the function, else 204 (versions cascade).
#[utoipa::path(
    delete,
    path = "/api/v1/functions/{id}",
    tag = "functions",
    operation_id = "functions_delete",
    summary = "Delete a function",
    description = "Deletes the function and all its versions. Refused with 409 `function-in-use` while a deployed flow references it (`args.flows` names them). Requires owner, admin or member role.",
    params(("id" = i64, Path, description = "Function id")),
    responses(
        (status = 204, description = "Deleted"),
        (status = 404, body = crate::openapi::ErrorDetail),
        (status = 409, description = "`function-in-use`: used by deployed flows (`args.flows`)", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn delete(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(id): Path<i64>,
) -> Result<Response> {
    let Some(f) = find_function(&ctx.db, &org, id).await? else {
        return Err(Error::NotFound);
    };
    let referencing = referencing_deployed_flows(&ctx.db, f.id)
        .await
        .map_err(|_| Error::InternalServerError)?;
    if !referencing.is_empty() {
        let flows: Vec<String> = referencing
            .iter()
            .map(|(flow, version_number)| format!("#{} {} v{version_number}", flow.id, flow.name))
            .collect();
        return Err(super::coded_error(
            StatusCode::CONFLICT,
            pnex_core::err_codes::FUNCTION_IN_USE,
            "Deployed flows use this function: stop them or remove it from their nodes first.",
            Some(serde_json::json!({ "flows": flows.join(", ") })),
        ));
    }
    f.delete(&ctx.db)
        .await
        .map_err(|_| Error::InternalServerError)?;
    Ok(StatusCode::NO_CONTENT.into_response())
}

/// `POST /api/v1/functions/{id}/test` — viewer allowed (read-only
/// tuning tool). 503 `flow_runtime` if the runtime is not enabled or
/// unreachable; the script result travels as 200 (`ok:false` + `error`).
#[utoipa::path(
    post,
    path = "/api/v1/functions/{id}/test",
    tag = "functions",
    operation_id = "functions_test",
    summary = "Run a function once with test inputs",
    description = "Executes the pinned version (default: current) or the ad-hoc unsaved code in a sandboxed runtime process with the given inputs and simulated incoming `msg`. No persistent side effect. Any role (viewer included). A script error is a 200 with `ok: false` and `error`.",
    params(("id" = i64, Path, description = "Function id")),
    request_body = FunctionTestRequest,
    responses(
        (status = 200, body = pnex_core::FunctionTestResponse),
        (status = 404, body = crate::openapi::ErrorDetail),
        (status = 503, description = "`functions-runtime-disabled`, `flow-runtime` (`args.detail`: runtime unreachable, timeout, unreadable output; verbatim diagnostic) or server busy", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn test(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(id): Path<i64>,
    Json(req): Json<FunctionTestRequest>,
) -> Result<Response> {
    let Some(f) = find_function(&ctx.db, &org, id).await? else {
        return Err(Error::NotFound);
    };
    let settings = FlowSettings::from_config(&ctx.config);
    if !settings.enabled {
        return Err(runtime_disabled());
    }
    // Per-pod cap on short-lived runtime children (shared with the deploy
    // pre-flight check): 503 server-busy past the wait budget.
    let _permit = crate::services::compute_limits::runtime_check()
        .acquire()
        .await?;
    match spawn_function_test(&settings, &ctx.db, org.org.id, f.id, &req).await {
        Ok(resp) => Ok(format::json(resp).into_response()),
        // Runtime/engine diagnostic: the raw message embeds arbitrary data —
        // verbatim display is the documented exception (err_codes doctrine).
        Err(msg) => Err(flow_runtime_503(&msg)),
    }
}

/// `POST /api/v1/functions/validate` — compile-only validation of the code
/// (editor error bar, no execution). Viewer allowed,
/// no DB access: the body carries language + code. Same 503 guard as the
/// test (the runtime binary is required to compile).
#[utoipa::path(
    post,
    path = "/api/v1/functions/validate",
    tag = "functions",
    operation_id = "functions_validate",
    summary = "Compile-check function code",
    description = "Compiles the given JS/Starlark code without running it and returns diagnostics (line/column best-effort). No database access, no side effect. Any role. Never queued: 503 at once when the check pool is full.",
    request_body = FunctionValidateRequest,
    responses(
        (status = 200, body = pnex_core::FunctionValidateResponse),
        (status = 503, description = "`functions-runtime-disabled`, `flow-runtime` (`args.detail`: runtime unreachable, timeout, unreadable output; verbatim diagnostic) or server busy", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn validate(
    State(ctx): State<AppContext>,
    _org: OrgContext,
    Json(req): Json<FunctionValidateRequest>,
) -> Result<Response> {
    let settings = FlowSettings::from_config(&ctx.config);
    if !settings.enabled {
        return Err(runtime_disabled());
    }
    // Editor validation is debounced and retried on the next keystroke:
    // never queue it, reject at once when the runtime-check pool is full.
    let _permit = crate::services::compute_limits::runtime_check().try_acquire()?;
    match spawn_function_check(&settings, &req).await {
        Ok(resp) => Ok(format::json(resp).into_response()),
        // Runtime/engine diagnostic: verbatim display is the documented
        // exception (err_codes doctrine).
        Err(msg) => Err(flow_runtime_503(&msg)),
    }
}
