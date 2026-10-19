//! Ontology imports API (partner-watch.md D301, D303) under
//! `/api/v1/ontology/imports`. Org from the principal (R1); every write
//! needs `can_write` (R2), the type write roles still apply to what an
//! import writes (D188). Previews and applications run in the background:
//! the answer is the run (202), polled on `…/runs/{run_id}`.

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::Json;
use loco_rs::controller::format;
use loco_rs::prelude::*;
use pnex_core::ontology::import::{ImportApply, ObjectImportInput};
use serde::Deserialize;

use super::ontology::{onto_error, paged, parse_uuid, PageQuery};
use crate::auth::OrgContext;
use crate::services::ontology::imports;

/// A file sent for a preview, compressed (the decompressed cap is D301's).
const FILE_MAX: usize = 50 * 1024 * 1024;

pub fn routes() -> Routes {
    Routes::new()
        .prefix("/api/v1/ontology/imports")
        .add("/", get(list).post(create))
        .add("/{id}", get(one).put(update).delete(remove))
        .add("/{id}/runs", get(runs))
        .add("/{id}/runs/{run_id}", get(run))
        .add(
            "/{id}/dry-run",
            post(dry_run).layer(axum::extract::DefaultBodyLimit::max(FILE_MAX)),
        )
        .add("/{id}/apply", post(apply))
}

/// OpenAPI paths of this controller (merged by `crate::openapi::spec`).
pub fn openapi() -> utoipa::openapi::OpenApi {
    #[derive(utoipa::OpenApi)]
    #[openapi(paths(list, create, one, update, remove, runs, run, dry_run, apply))]
    struct Doc;
    <Doc as utoipa::OpenApi>::openapi()
}

fn reply<T: serde::Serialize>(
    status: StatusCode,
    r: crate::services::ontology::Result<T>,
) -> Result<Response> {
    match r {
        Ok(v) => Ok((status, format::json(v)).into_response()),
        Err(e) => onto_error(e),
    }
}

#[utoipa::path(
    get,
    path = "/api/v1/ontology/imports",
    tag = "ontology",
    operation_id = "ontology_imports_list",
    summary = "List the imports",
    description = "The org's declarative imports (mapping, source, mode, schedule) with their last run. Paginated (`limit`/`offset`). Any org member.",
    params(PageQuery),
    responses(
        (status = 200, body = pnex_core::Paginated<pnex_core::ontology::import::ObjectImportView>)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn list(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Query(q): Query<PageQuery>,
) -> Result<Response> {
    paged(
        "/api/v1/ontology/imports",
        &[],
        &q,
        imports::list(&ctx.db, org.org.id).await,
    )
}

#[utoipa::path(
    post,
    path = "/api/v1/ontology/imports",
    tag = "ontology",
    operation_id = "ontology_imports_create",
    summary = "Create an import",
    description = "Creates a declarative import: a YAML mapping checked against the org schema, a source (`file` sent with each preview, `url`, or `api`), a mode (`incremental` or `snapshot`) and, for URL and API sources, an optional recurrence (scheduled runs apply without preview). Org writers (owner, admin, member).",
    request_body = ObjectImportInput,
    responses(
        (status = 201, body = pnex_core::ontology::import::ObjectImportView),
        (status = 400, description = "Invalid input or mapping (field → token)", body = crate::openapi::ErrorDetail),
        (status = 403, description = "`ontology-write-forbidden` (viewer)", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn create(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Json(input): Json<ObjectImportInput>,
) -> Result<Response> {
    reply(
        StatusCode::CREATED,
        imports::create(&ctx.db, org.org.id, Some(org.auth.user.id), &input, None).await,
    )
}

#[utoipa::path(
    get,
    path = "/api/v1/ontology/imports/{id}",
    tag = "ontology",
    operation_id = "ontology_imports_get",
    summary = "Get an import",
    description = "One import with its last run. Any org member.",
    params(("id" = String, Path, format = "uuid", description = "Import id")),
    responses(
        (status = 200, body = pnex_core::ontology::import::ObjectImportView),
        (status = 404, description = "`ontology-not-found`", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn one(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(id): Path<String>,
) -> Result<Response> {
    reply(
        StatusCode::OK,
        imports::get(&ctx.db, org.org.id, parse_uuid(&id)?).await,
    )
}

#[utoipa::path(
    put,
    path = "/api/v1/ontology/imports/{id}",
    tag = "ontology",
    operation_id = "ontology_imports_update",
    summary = "Update an import",
    description = "Replaces the import on top of `expected_version` (409 otherwise); a pending preview of the previous version can no longer be applied. Org writers.",
    params(("id" = String, Path, format = "uuid", description = "Import id")),
    request_body = ObjectImportInput,
    responses(
        (status = 200, body = pnex_core::ontology::import::ObjectImportView),
        (status = 400, description = "Invalid input or mapping (field → token)", body = crate::openapi::ErrorDetail),
        (status = 403, description = "`ontology-write-forbidden` (viewer)", body = crate::openapi::ErrorDetail),
        (status = 404, description = "`ontology-not-found`", body = crate::openapi::ErrorDetail),
        (status = 409, description = "`ontology-version-conflict`", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn update(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(id): Path<String>,
    Json(input): Json<ObjectImportInput>,
) -> Result<Response> {
    reply(
        StatusCode::OK,
        imports::update(&ctx.db, org.org.id, parse_uuid(&id)?, &input).await,
    )
}

#[utoipa::path(
    delete,
    path = "/api/v1/ontology/imports/{id}",
    tag = "ontology",
    operation_id = "ontology_imports_delete",
    summary = "Delete an import",
    description = "Deletes the import and its run journal; the objects and links it wrote stay. Org writers.",
    params(("id" = String, Path, format = "uuid", description = "Import id")),
    responses(
        (status = 204, description = "Deleted"),
        (status = 403, description = "`ontology-write-forbidden` (viewer)", body = crate::openapi::ErrorDetail),
        (status = 404, description = "`ontology-not-found`", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn remove(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(id): Path<String>,
) -> Result<Response> {
    match imports::delete(&ctx.db, org.org.id, parse_uuid(&id)?).await {
        Ok(()) => Ok(StatusCode::NO_CONTENT.into_response()),
        Err(e) => onto_error(e),
    }
}

#[utoipa::path(
    get,
    path = "/api/v1/ontology/imports/{id}/runs",
    tag = "ontology",
    operation_id = "ontology_imports_runs_list",
    summary = "List the runs of an import",
    description = "The last 50 runs, newest first: state (`running`, `preview`, `applied`, `refused`, `failed`, `stale`), refusal code, counters. Paginated (`limit`/`offset`). Any org member.",
    params(("id" = String, Path, format = "uuid", description = "Import id"), PageQuery),
    responses(
        (status = 200, body = pnex_core::Paginated<pnex_core::ontology::import::ImportRunView>),
        (status = 404, description = "`ontology-not-found`", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn runs(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(id): Path<String>,
    Query(q): Query<PageQuery>,
) -> Result<Response> {
    let id = parse_uuid(&id)?;
    paged(
        &format!("/api/v1/ontology/imports/{id}/runs"),
        &[],
        &q,
        imports::runs(&ctx.db, org.org.id, id).await,
    )
}

#[utoipa::path(
    get,
    path = "/api/v1/ontology/imports/{id}/runs/{run_id}",
    tag = "ontology",
    operation_id = "ontology_imports_runs_get",
    summary = "Get a run and its difference",
    description = "A run and, while it is a preview, its difference: objects created, updated (old → new), closed, links opened or updated, errors and warnings by row and column (each list cut to 500 entries; the counters hold the totals). Any org member.",
    params(
        ("id" = String, Path, format = "uuid", description = "Import id"),
        ("run_id" = String, Path, format = "uuid", description = "Run id")
    ),
    responses(
        (status = 200, body = pnex_core::ontology::import::ImportPreview),
        (status = 404, description = "`ontology-not-found`", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn run(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path((id, run_id)): Path<(String, String)>,
) -> Result<Response> {
    reply(
        StatusCode::OK,
        imports::run(&ctx.db, org.org.id, parse_uuid(&id)?, parse_uuid(&run_id)?).await,
    )
}

#[derive(Debug, Deserialize, utoipa::IntoParams)]
#[into_params(parameter_in = Query)]
struct DryRunQuery {
    /// Name of the file sent (its extension gives the format when the
    /// mapping does not).
    filename: Option<String>,
}

#[utoipa::path(
    post,
    path = "/api/v1/ontology/imports/{id}/dry-run",
    tag = "ontology",
    operation_id = "ontology_imports_dry_run",
    summary = "Preview an import",
    description = "Starts a preview: the source (the file sent as the raw body for a `file` import, max 50 MB; fetched for `url` and `api`) is read and its difference computed in the background without writing anything. Answers the `running` run (202); poll `GET …/runs/{run_id}`. Org writers.",
    params(("id" = String, Path, format = "uuid", description = "Import id"), DryRunQuery),
    request_body(content = Vec<u8>, content_type = "application/octet-stream", description = "The file (file imports only)"),
    responses(
        (status = 202, body = pnex_core::ontology::import::ImportRunView),
        (status = 400, description = "`file` required for a file import (field → token)", body = crate::openapi::ErrorDetail),
        (status = 403, description = "`ontology-write-forbidden` (viewer)", body = crate::openapi::ErrorDetail),
        (status = 404, description = "`ontology-not-found`", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn dry_run(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(id): Path<String>,
    Query(q): Query<DryRunQuery>,
    body: axum::body::Bytes,
) -> Result<Response> {
    let file = (!body.is_empty()).then(|| (body.to_vec(), q.filename.unwrap_or_default()));
    reply(
        StatusCode::ACCEPTED,
        imports::dry_run(
            &ctx.db,
            org.org.id,
            Some(org.auth.user.id),
            parse_uuid(&id)?,
            file,
        )
        .await,
    )
}

#[utoipa::path(
    post,
    path = "/api/v1/ontology/imports/{id}/apply",
    tag = "ontology",
    operation_id = "ontology_imports_apply",
    summary = "Apply a preview",
    description = "Writes exactly the difference of the preview `run_id`, in one transaction, on the import version `expected_version` and the object versions the preview read: anything changed since refuses the whole run (`import-run-stale`). A preview with row errors is refused (`import-has-errors`). Runs in the background: answers the `running` run (202). Org writers; the type write roles apply.",
    params(("id" = String, Path, format = "uuid", description = "Import id")),
    request_body = ImportApply,
    responses(
        (status = 202, body = pnex_core::ontology::import::ImportRunView),
        (status = 400, description = "Invalid `run_id` (field → token)", body = crate::openapi::ErrorDetail),
        (status = 403, description = "`ontology-write-forbidden` (viewer)", body = crate::openapi::ErrorDetail),
        (status = 404, description = "`ontology-not-found`", body = crate::openapi::ErrorDetail),
        (status = 409, description = "`ontology-version-conflict`, `import-run-stale`, `import-has-errors`", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn apply(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(id): Path<String>,
    Json(input): Json<ImportApply>,
) -> Result<Response> {
    reply(
        StatusCode::ACCEPTED,
        imports::apply(
            &ctx.db,
            org.org.id,
            Some(org.auth.user.id),
            org.role,
            parse_uuid(&id)?,
            &input,
        )
        .await,
    )
}
