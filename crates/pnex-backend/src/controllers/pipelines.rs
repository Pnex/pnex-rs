//! Pipelines API (data-pipelines.md D571, D573): documents, runs, lineage. Org
//! from the principal (R1), permissions applied by `OrgContext` (R2).

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::Json;
use loco_rs::controller::{format, ErrorDetail};
use loco_rs::prelude::*;
use pnex_core::err_codes;
use pnex_core::pipelines::{PipelineInput, PipelineUpdate};
use uuid::Uuid;

use crate::auth::OrgContext;
use crate::controllers::pagination::{self, PageParams};
use crate::services::pipelines::{self, PipelineError};

pub fn routes() -> Routes {
    Routes::new()
        .prefix("/api/v1")
        .add("/pipelines", get(list).post(create))
        .add("/pipelines/{id}", get(one).put(update).delete(remove))
        .add("/pipelines/{id}/versions", get(versions))
        .add("/pipelines/{id}/runs", post(start_run).get(runs))
        .add("/pipelines/{id}/runs/{run_id}", get(run))
        .add("/datasets/{id}/lineage", get(lineage))
}

/// OpenAPI paths of this controller (merged by `crate::openapi::spec`).
pub fn openapi() -> utoipa::openapi::OpenApi {
    #[derive(utoipa::OpenApi)]
    #[openapi(paths(
        list, one, create, update, remove, versions, start_run, runs, run, lineage
    ))]
    struct Doc;
    <Doc as utoipa::OpenApi>::openapi()
}

fn pipeline_error(e: PipelineError) -> Result<Response> {
    let detail =
        |status, code: &str, text: &str| Error::CustomError(status, ErrorDetail::new(code, text));
    Err(match e {
        PipelineError::Invalid { field, token } => return Ok(super::field_error(&field, &token)),
        PipelineError::Document(v) => {
            return Ok(super::violations_error(
                err_codes::PIPELINE_DOCUMENT_INVALID,
                v,
            ))
        }
        PipelineError::NotFound => detail(
            StatusCode::NOT_FOUND,
            err_codes::PIPELINE_NOT_FOUND,
            "Pipeline not found.",
        ),
        PipelineError::RunNotFound => detail(
            StatusCode::NOT_FOUND,
            err_codes::PIPELINE_RUN_NOT_FOUND,
            "Run not found.",
        ),
        PipelineError::NameTaken => detail(
            StatusCode::CONFLICT,
            err_codes::PIPELINE_NAME_TAKEN,
            "A pipeline of the org already has this name.",
        ),
        PipelineError::RunActive => detail(
            StatusCode::CONFLICT,
            err_codes::PIPELINE_RUN_ACTIVE,
            "A run of this pipeline is queued or running.",
        ),
        PipelineError::VersionConflict { current } => super::coded_error(
            StatusCode::CONFLICT,
            err_codes::PIPELINE_VERSION_CONFLICT,
            "The pipeline changed since it was read; reload it.",
            Some(serde_json::json!({ "current": current })),
        ),
        PipelineError::Db(e) => {
            tracing::error!(error = %e, "pipeline database error");
            Error::InternalServerError
        }
    })
}

/// Page of a list (`limit`/`offset`, D14).
#[derive(serde::Deserialize, utoipa::IntoParams)]
#[into_params(parameter_in = Query)]
struct PageQuery {
    #[param(value_type = Option<i64>)]
    limit: Option<String>,
    #[param(value_type = Option<i64>)]
    offset: Option<String>,
}

impl PageQuery {
    fn page(&self) -> PageParams {
        PageParams::from(self.limit.as_deref(), self.offset.as_deref())
    }
}

/// `GET /api/v1/pipelines`
#[utoipa::path(
    get,
    path = "/api/v1/pipelines",
    tag = "pipelines",
    operation_id = "pipelines_list",
    summary = "List the pipelines",
    description = "Pipelines of the org by name, with their current document. Requires `pipelines:read`. Paginated (`limit`/`offset`).",
    params(PageQuery),
    responses(
        (status = 200, body = pnex_core::Paginated<pnex_core::pipelines::Pipeline>),
        (status = 403, description = "`access-denied`", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn list(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Query(q): Query<PageQuery>,
) -> Result<Response> {
    let page = q.page();
    match pipelines::list(&ctx.db, org.org.id, page.limit, page.offset).await {
        Ok((count, rows)) => format::json(pagination::envelope(
            "/api/v1/pipelines",
            &[],
            page,
            count,
            rows,
        )),
        Err(e) => pipeline_error(e),
    }
}

/// `POST /api/v1/pipelines`
#[utoipa::path(
    post,
    path = "/api/v1/pipelines",
    tag = "pipelines",
    operation_id = "pipelines_create",
    summary = "Create a pipeline",
    description = "Creates a pipeline from a YAML document of SQL transforms, each with its input datasets and its output dataset. Requires `pipelines:write`.",
    request_body = pnex_core::pipelines::PipelineInput,
    responses(
        (status = 201, body = pnex_core::pipelines::Pipeline),
        (status = 400, description = "`pipeline-document-invalid` (`errors.violations`: `path`, `token`) or `field-invalid` on `name`", body = crate::openapi::ErrorDetail),
        (status = 403, description = "`access-denied`", body = crate::openapi::ErrorDetail),
        (status = 409, description = "`pipeline-name-taken`", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn create(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Json(input): Json<PipelineInput>,
) -> Result<Response> {
    match pipelines::create(&ctx.db, org.org.id, &input.name, &input.document).await {
        Ok(p) => Ok((StatusCode::CREATED, Json(p)).into_response()),
        Err(e) => pipeline_error(e),
    }
}

/// `GET /api/v1/pipelines/{id}`
#[utoipa::path(
    get,
    path = "/api/v1/pipelines/{id}",
    tag = "pipelines",
    operation_id = "pipelines_get",
    summary = "Get a pipeline",
    description = "One pipeline with its current document. Requires `pipelines:read`.",
    params(("id" = String, Path, format = "uuid", description = "Pipeline id (UUID)")),
    responses(
        (status = 200, body = pnex_core::pipelines::Pipeline),
        (status = 403, description = "`access-denied`", body = crate::openapi::ErrorDetail),
        (status = 404, description = "`pipeline-not-found`", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn one(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(id): Path<Uuid>,
) -> Result<Response> {
    match pipelines::get(&ctx.db, org.org.id, id).await {
        Ok(p) => format::json(p),
        Err(e) => pipeline_error(e),
    }
}

/// `PUT /api/v1/pipelines/{id}`
#[utoipa::path(
    put,
    path = "/api/v1/pipelines/{id}",
    tag = "pipelines",
    operation_id = "pipelines_update",
    summary = "Save a new version of a pipeline",
    description = "Replaces the document and adds a version, on the version that was read (`expected_version`). A run keeps the version it started with. Requires `pipelines:write`.",
    params(("id" = String, Path, format = "uuid", description = "Pipeline id (UUID)")),
    request_body = pnex_core::pipelines::PipelineUpdate,
    responses(
        (status = 200, body = pnex_core::pipelines::Pipeline),
        (status = 400, description = "`pipeline-document-invalid` (`errors.violations`: `path`, `token`)", body = crate::openapi::ErrorDetail),
        (status = 403, description = "`access-denied`", body = crate::openapi::ErrorDetail),
        (status = 404, description = "`pipeline-not-found`", body = crate::openapi::ErrorDetail),
        (status = 409, description = "`pipeline-version-conflict` (args `current`)", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn update(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(id): Path<Uuid>,
    Json(input): Json<PipelineUpdate>,
) -> Result<Response> {
    let saved = pipelines::update(
        &ctx.db,
        org.org.id,
        id,
        &input.document,
        input.expected_version,
    )
    .await;
    match saved {
        Ok(p) => format::json(p),
        Err(e) => pipeline_error(e),
    }
}

/// `DELETE /api/v1/pipelines/{id}`
#[utoipa::path(
    delete,
    path = "/api/v1/pipelines/{id}",
    tag = "pipelines",
    operation_id = "pipelines_delete",
    summary = "Delete a pipeline",
    description = "Removes a pipeline, its versions, its runs and the lineage they wrote; the datasets it computed stay. Requires `pipelines:delete`.",
    params(("id" = String, Path, format = "uuid", description = "Pipeline id (UUID)")),
    responses(
        (status = 204, description = "Deleted"),
        (status = 403, description = "`access-denied`", body = crate::openapi::ErrorDetail),
        (status = 404, description = "`pipeline-not-found`", body = crate::openapi::ErrorDetail),
        (status = 409, description = "`pipeline-run-active`", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn remove(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(id): Path<Uuid>,
) -> Result<Response> {
    match pipelines::delete(&ctx.db, org.org.id, id).await {
        Ok(()) => Ok(StatusCode::NO_CONTENT.into_response()),
        Err(e) => pipeline_error(e),
    }
}

/// `GET /api/v1/pipelines/{id}/versions`
#[utoipa::path(
    get,
    path = "/api/v1/pipelines/{id}/versions",
    tag = "pipelines",
    operation_id = "pipelines_versions_list",
    summary = "List the versions of a pipeline",
    description = "Saved documents, newest first. Requires `pipelines:read`. Paginated (`limit`/`offset`).",
    params(
        ("id" = String, Path, format = "uuid", description = "Pipeline id (UUID)"),
        PageQuery
    ),
    responses(
        (status = 200, body = pnex_core::Paginated<pnex_core::pipelines::PipelineVersion>),
        (status = 403, description = "`access-denied`", body = crate::openapi::ErrorDetail),
        (status = 404, description = "`pipeline-not-found`", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn versions(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(id): Path<Uuid>,
    Query(q): Query<PageQuery>,
) -> Result<Response> {
    let page = q.page();
    match pipelines::versions(&ctx.db, org.org.id, id, page.limit, page.offset).await {
        Ok((count, rows)) => format::json(pagination::envelope(
            &format!("/api/v1/pipelines/{id}/versions"),
            &[],
            page,
            count,
            rows,
        )),
        Err(e) => pipeline_error(e),
    }
}

/// `POST /api/v1/pipelines/{id}/runs`
#[utoipa::path(
    post,
    path = "/api/v1/pipelines/{id}/runs",
    tag = "pipelines",
    operation_id = "pipelines_runs_start",
    summary = "Run a pipeline",
    description = "Queues a run of the current version for the worker; read its state with `GET /pipelines/{id}/runs/{run_id}`. Requires `pipelines:run`, and `datasets:read` and `datasets:write` since the run reads and writes datasets.",
    params(("id" = String, Path, format = "uuid", description = "Pipeline id (UUID)")),
    responses(
        (status = 202, body = pnex_core::pipelines::PipelineRun),
        (status = 403, description = "`access-denied`", body = crate::openapi::ErrorDetail),
        (status = 404, description = "`pipeline-not-found`", body = crate::openapi::ErrorDetail),
        (status = 409, description = "`pipeline-run-active`", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn start_run(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(id): Path<Uuid>,
) -> Result<Response> {
    for needed in ["datasets:read", "datasets:write"] {
        if !org.can(needed) {
            return Err(crate::auth::access_denied(needed));
        }
    }
    match pipelines::start_run(&ctx.db, org.org.id, id).await {
        Ok(r) => Ok((StatusCode::ACCEPTED, Json(r)).into_response()),
        Err(e) => pipeline_error(e),
    }
}

/// `GET /api/v1/pipelines/{id}/runs`
#[utoipa::path(
    get,
    path = "/api/v1/pipelines/{id}/runs",
    tag = "pipelines",
    operation_id = "pipelines_runs_list",
    summary = "List the runs of a pipeline",
    description = "Runs of a pipeline, newest first, failed ones with their code. Requires `pipelines:read`. Paginated (`limit`/`offset`).",
    params(
        ("id" = String, Path, format = "uuid", description = "Pipeline id (UUID)"),
        PageQuery
    ),
    responses(
        (status = 200, body = pnex_core::Paginated<pnex_core::pipelines::PipelineRun>),
        (status = 403, description = "`access-denied`", body = crate::openapi::ErrorDetail),
        (status = 404, description = "`pipeline-not-found`", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn runs(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(id): Path<Uuid>,
    Query(q): Query<PageQuery>,
) -> Result<Response> {
    let page = q.page();
    match pipelines::runs(&ctx.db, org.org.id, id, page.limit, page.offset).await {
        Ok((count, rows)) => format::json(pagination::envelope(
            &format!("/api/v1/pipelines/{id}/runs"),
            &[],
            page,
            count,
            rows,
        )),
        Err(e) => pipeline_error(e),
    }
}

/// `GET /api/v1/pipelines/{id}/runs/{run_id}`
#[utoipa::path(
    get,
    path = "/api/v1/pipelines/{id}/runs/{run_id}",
    tag = "pipelines",
    operation_id = "pipelines_runs_get",
    summary = "Get a run of a pipeline",
    description = "State of one run, with its failure code and arguments. Requires `pipelines:read`.",
    params(
        ("id" = String, Path, format = "uuid", description = "Pipeline id (UUID)"),
        ("run_id" = String, Path, format = "uuid", description = "Run id (UUID)")
    ),
    responses(
        (status = 200, body = pnex_core::pipelines::PipelineRun),
        (status = 403, description = "`access-denied`", body = crate::openapi::ErrorDetail),
        (status = 404, description = "`pipeline-run-not-found`", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn run(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path((id, run_id)): Path<(Uuid, Uuid)>,
) -> Result<Response> {
    match pipelines::get_run(&ctx.db, org.org.id, id, run_id).await {
        Ok(r) => format::json(r),
        Err(e) => pipeline_error(e),
    }
}

/// `GET /api/v1/datasets/{id}/lineage`
#[utoipa::path(
    get,
    path = "/api/v1/datasets/{id}/lineage",
    tag = "datasets",
    operation_id = "datasets_lineage",
    summary = "Get the lineage of a dataset",
    description = "Datasets it was computed from and datasets computed from it, as written by the runs, and whether an input has changed since. Requires `datasets:read`.",
    params(("id" = String, Path, format = "uuid", description = "Dataset id (UUID)")),
    responses(
        (status = 200, body = pnex_core::pipelines::DatasetLineage),
        (status = 403, description = "`access-denied`", body = crate::openapi::ErrorDetail),
        (status = 404, description = "`dataset-not-found`", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn lineage(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(id): Path<Uuid>,
) -> Result<Response> {
    let Ok(dataset) = crate::services::datasets::get(&ctx.db, org.org.id, id).await else {
        return Err(Error::CustomError(
            StatusCode::NOT_FOUND,
            ErrorDetail::new(err_codes::DATASET_NOT_FOUND, "Dataset not found."),
        ));
    };
    match pipelines::lineage(&ctx.db, &dataset).await {
        Ok(l) => format::json(l),
        Err(e) => pipeline_error(e.into()),
    }
}
