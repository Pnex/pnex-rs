//! Pipelines API (pipelines.md D934, D571, D573): an org's own engine
//! pipelines, their runs, lineage. Org from the principal (R1), permissions
//! applied by `OrgContext` (R2).

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::routing::{get, post, put};
use axum::Json;
use loco_rs::controller::{format, ErrorDetail};
use loco_rs::prelude::*;
use pnex_core::err_codes;
use pnex_core::pipelines::{
    BuildInput, BuildStep, BuildTargetKind, PipelineInput, PipelineSchedule, PipelineUpdate,
};
use uuid::Uuid;

use crate::auth::OrgContext;
use crate::controllers::pagination::{self, PageParams};
use crate::services::data_builds::{self, BuildError};
use crate::services::pipelines::{self, PipelineError};

pub fn routes() -> Routes {
    Routes::new()
        .prefix("/api/v1")
        .add("/pipelines", get(list).post(create))
        .add("/pipelines/validate", post(validate))
        .add("/pipelines/probe", post(probe))
        .add("/pipelines/{id}", get(one).put(update).delete(remove))
        .add("/pipelines/{id}/versions", get(versions))
        .add("/pipelines/{id}/runs", post(start_run).get(runs))
        .add("/pipelines/{id}/runs/{run_id}", get(run))
        .add("/pipelines/{id}/schedule", put(schedule))
        .add("/data-builds", post(build_start).get(builds))
        .add("/data-builds/plan", get(build_plan))
        .add("/data-builds/{id}", get(build))
        .add("/datasets/{id}/lineage", get(lineage))
}

/// OpenAPI paths of this controller (merged by `crate::openapi::spec`).
pub fn openapi() -> utoipa::openapi::OpenApi {
    #[derive(utoipa::OpenApi)]
    #[openapi(paths(
        list,
        one,
        create,
        validate,
        probe,
        update,
        remove,
        versions,
        start_run,
        runs,
        run,
        schedule,
        builds,
        build,
        build_plan,
        build_start,
        lineage
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
        PipelineError::MarkingRequired(missing) => super::coded_error(
            StatusCode::FORBIDDEN,
            err_codes::MARKING_REQUIRED,
            "A clearance is missing for a marking of a table the pipeline reads.",
            Some(serde_json::json!({ "markings": missing })),
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

fn build_error(e: BuildError) -> Result<Response> {
    let not_found = |code: &str, text: &str| {
        Error::CustomError(StatusCode::NOT_FOUND, ErrorDetail::new(code, text))
    };
    Err(match e {
        BuildError::NotFound => not_found(err_codes::BUILD_NOT_FOUND, "Build not found."),
        BuildError::TargetNotFound => not_found(
            err_codes::BUILD_NOT_FOUND,
            "No pipeline or import of the org has this id.",
        ),
        BuildError::Plan(e) => {
            let (code, args) = data_builds::plan_refusal(&e);
            super::coded_error(
                StatusCode::CONFLICT,
                code,
                "The dependencies of this target cannot be ordered.",
                Some(args),
            )
        }
        BuildError::Db(e) => {
            tracing::error!(error = %e, "build database error");
            Error::InternalServerError
        }
    })
}

/// A build runs what its steps run: its caller needs each right (R2).
fn deny_without_rights(org: &OrgContext, steps: &[BuildStep]) -> Result<()> {
    match data_builds::rights_of(steps)
        .into_iter()
        .find(|p| !org.can(p))
    {
        Some(p) => Err(crate::auth::access_denied(p)),
        None => Ok(()),
    }
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
    description = "Creates a pipeline from a YAML engine document (steps such as `fetch`, `read`, `sql`, `write`), after its `tests:` pass on the answers they hold. A navigation (`browse`) plays in the starter's browser agent. It may not share into another org, read pack settings, nor set its own schedule. A site rule (`key: site_rule`) is a pipeline too: its `rule` is answered. Requires `pipelines:write`.",
    request_body = pnex_core::pipelines::PipelineInput,
    responses(
        (status = 201, body = pnex_core::pipelines::Pipeline),
        (status = 400, description = "`pipeline-document-invalid` (`errors.violations`: `path`, `token`; a failed test is under `tests.<n>`) or `field-invalid` on `name`", body = crate::openapi::ErrorDetail),
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
    let author = org.auth.user.id;
    match pipelines::create(&ctx, org.org.id, &input.name, &input.document, author).await {
        Ok(p) => Ok((StatusCode::CREATED, Json(p)).into_response()),
        Err(e) => pipeline_error(e),
    }
}

/// `POST /api/v1/pipelines/validate`
#[utoipa::path(
    post,
    path = "/api/v1/pipelines/validate",
    tag = "pipelines",
    operation_id = "pipelines_validate",
    summary = "Check a pipeline document",
    description = "Checks a YAML document and plays its `tests:` without saving it, and answers the tables it reads and writes and the rights its run needs. Loops through other pipelines are only found at save. Requires `pipelines:read`.",
    request_body = pnex_core::pipelines::PipelineDraft,
    responses(
        (status = 200, body = pnex_core::pipelines::PipelineCheck),
        (status = 400, description = "`pipeline-document-invalid` (`errors.violations`: `path`, `token`)", body = crate::openapi::ErrorDetail),
        (status = 403, description = "`access-denied`", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn validate(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Json(input): Json<pnex_core::pipelines::PipelineDraft>,
) -> Result<Response> {
    match pipelines::check(&ctx, org.org.id, &input.document).await {
        Ok(doc) => format::json(doc),
        Err(e) => pipeline_error(e),
    }
}

/// `POST /api/v1/pipelines/probe`
#[utoipa::path(
    post,
    path = "/api/v1/pipelines/probe",
    tag = "pipelines",
    operation_id = "pipelines_probe",
    summary = "Probe a pipeline document",
    description = "Reads a YAML document up to the step whose `as` is `step`, on one row, without saving it and without writes: its write steps are left out and nothing of the probe is kept. Answers what each step did and the first 20 rows of the last one as text. A document with an `asked` axis takes its rows in `rows`, as a run does. A navigation (`browse`) before the step is refused. Requires `pipelines:run`, the table rights of a run (`datasets:read`, `datasets:write`, `connectors:use` for a connector step) and a clearance for every marking of the tables it reads.",
    request_body = pnex_core::pipelines::PipelineProbe,
    responses(
        (status = 200, body = pnex_core::pipelines::PipelineProbeResult),
        (status = 400, description = "`pipeline-document-invalid` (`errors.violations`: `path`, `token`); `field-invalid`: `step` unknown or after a navigation (`browse`), `rows` as for a run", body = crate::openapi::ErrorDetail),
        (status = 403, description = "`access-denied`, `marking-required` (`errors.args.markings`)", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn probe(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Json(input): Json<pnex_core::pipelines::PipelineProbe>,
) -> Result<Response> {
    let doc = match pipelines::probed(&input.document, &input.step) {
        Ok(d) => d,
        Err(e) => return pipeline_error(e),
    };
    if let Some(p) = doc.rights().into_iter().find(|p| !org.can(p)) {
        return Err(crate::auth::access_denied(p));
    }
    if let Err((field, token)) = run_rows(doc.asked_axis(), &input.rows) {
        return Ok(super::field_error(&field, &token));
    }
    let rows = input
        .rows
        .into_iter()
        .map(|r| r.into_iter().map(|(k, v)| (k, v.into())).collect())
        .collect();
    match pipelines::probe(
        &ctx,
        org.org.id,
        org.auth.user.id,
        &input.document,
        &doc,
        rows,
    )
    .await
    {
        Ok(r) => format::json(r),
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
        &ctx,
        org.org.id,
        id,
        &input.document,
        input.expected_version,
        org.auth.user.id,
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
    description = "Queues a run of the current version; read its state with `GET /pipelines/{id}/runs/{run_id}`. A pipeline whose enumerate step has an `asked` axis (a site rule) takes its rows in `rows` (one row per value: the axis column and its asked columns), at most 100; any other pipeline refuses them. Body optional. Requires `pipelines:run`, the rights its writes need (`datasets:read` and `datasets:write` always; `ontology:write`, `time-ranges:write`, `proposals:file` or `media:write` for those targets), and a clearance for every marking of the tables it reads. The run checks them again when it starts.",
    params(("id" = String, Path, format = "uuid", description = "Pipeline id (UUID)")),
    request_body = Option<pnex_core::pipelines::PipelineRunStart>,
    responses(
        (status = 202, body = pnex_core::pipelines::PipelineRun),
        (status = 400, description = "`field-invalid`: `rows` required, refused or too many; `rows.<n>.<column>` missing, unknown or too long", body = crate::openapi::ErrorDetail),
        (status = 403, description = "`access-denied`, `marking-required` (`errors.args.markings`)", body = crate::openapi::ErrorDetail),
        (status = 404, description = "`pipeline-not-found`", body = crate::openapi::ErrorDetail),
        (status = 409, description = "`pipeline-run-active`", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn start_run(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(id): Path<Uuid>,
    // Read by hand: `Option<Json<_>>` refuses an empty body sent with a JSON
    // content type, and a run usually has no body.
    raw: axum::body::Bytes,
) -> Result<Response> {
    let current = match pipelines::get(&ctx.db, org.org.id, id).await {
        Ok(p) => p,
        Err(e) => return pipeline_error(e),
    };
    let doc = pipelines::checked(&current.document).ok();
    // A document saved checked: one of an older format needs the table rights only.
    let rights = doc
        .as_ref()
        .map(|d| d.rights())
        .unwrap_or_else(|| vec!["datasets:read", "datasets:write"]);
    if let Some(p) = rights.into_iter().find(|p| !org.can(p)) {
        return Err(crate::auth::access_denied(p));
    }
    let rows = match raw.is_empty() {
        true => Vec::new(),
        false => match serde_json::from_slice::<pnex_core::pipelines::PipelineRunStart>(&raw) {
            Ok(b) => b.rows,
            Err(_) => return Ok(super::field_error("rows", "invalid")),
        },
    };
    if let Err((field, token)) = run_rows(doc.as_ref().and_then(|d| d.asked_axis()), &rows) {
        return Ok(super::field_error(&field, &token));
    }
    let rows = rows
        .into_iter()
        .map(|r| r.into_iter().map(|(k, v)| (k, v.into())).collect())
        .collect();
    match pipelines::start_run(&ctx.db, org.org.id, id, org.auth.user.id, rows).await {
        Ok(r) => Ok((StatusCode::ACCEPTED, Json(r)).into_response()),
        Err(e) => pipeline_error(e),
    }
}

/// Checks the rows a start gives against the document's `asked` axis:
/// `Err((field, token))` on the first fault.
fn run_rows(
    axis: Option<&pnex_core::pipeline::doc::Axis>,
    rows: &[std::collections::BTreeMap<String, String>],
) -> std::result::Result<(), (String, String)> {
    use pnex_core::pipelines::RUN_ROWS_MAX;
    let Some(axis) = axis else {
        return match rows.is_empty() {
            true => Ok(()),
            false => Err(("rows".into(), "invalid".into())),
        };
    };
    if rows.is_empty() {
        return Err(("rows".into(), "required".into()));
    }
    if rows.len() > RUN_ROWS_MAX {
        return Err(("rows".into(), format!("max_items:{RUN_ROWS_MAX}")));
    }
    let asked = axis.asked.as_deref().unwrap_or_default();
    for (n, row) in rows.iter().enumerate() {
        if row.get(&axis.as_).is_none_or(|v| v.trim().is_empty()) {
            return Err((format!("rows.{n}.{}", axis.as_), "required".into()));
        }
        for (k, v) in row {
            if *k != axis.as_ && !asked.contains(k) {
                return Err((format!("rows.{n}.{k}"), "invalid".into()));
            }
            // Same bound as a navigation's parameters.
            if v.chars().count() > 500 {
                return Err((format!("rows.{n}.{k}"), "max_length:500".into()));
            }
        }
    }
    Ok(())
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

/// `PUT /api/v1/pipelines/{id}/schedule`
#[utoipa::path(
    put,
    path = "/api/v1/pipelines/{id}/schedule",
    tag = "pipelines",
    operation_id = "pipelines_schedule_set",
    summary = "Schedule a pipeline",
    description = "Sets when the pipeline and what it depends on run on their own, in order; `null` stops it. The caller answers for these runs: each one checks they may still run it. Requires `pipelines:run`, plus to set one `datasets:read` and `datasets:write`, and `ontology:write` when a step is an import.",
    params(("id" = String, Path, format = "uuid", description = "Pipeline id (UUID)")),
    request_body = pnex_core::pipelines::PipelineSchedule,
    responses(
        (status = 200, body = pnex_core::pipelines::Pipeline),
        (status = 400, description = "`field-invalid` on `recurrence`", body = crate::openapi::ErrorDetail),
        (status = 403, description = "`access-denied`", body = crate::openapi::ErrorDetail),
        (status = 404, description = "`pipeline-not-found`", body = crate::openapi::ErrorDetail),
        (status = 409, description = "`build-cycle`, `build-dependency-missing` or `build-too-long`", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn schedule(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(id): Path<Uuid>,
    Json(input): Json<PipelineSchedule>,
) -> Result<Response> {
    // What it will run unattended, the caller must be able to run now.
    if input.recurrence.is_some() {
        let kind = BuildTargetKind::Pipeline;
        match data_builds::steps_of(&ctx.db, org.org.id, kind, id).await {
            Ok((_, steps)) => deny_without_rights(&org, &steps)?,
            Err(BuildError::TargetNotFound) => {} // Answered below, as for any route.
            Err(e) => return build_error(e),
        }
    }
    let recurrence = input.recurrence.as_ref();
    match pipelines::schedule(&ctx.db, org.org.id, id, recurrence, org.auth.user.id).await {
        Ok(p) => format::json(p),
        Err(e) => pipeline_error(e),
    }
}

/// `GET /api/v1/data-builds`
#[utoipa::path(
    get,
    path = "/api/v1/data-builds",
    tag = "pipelines",
    operation_id = "data_builds_list",
    summary = "List the chained runs",
    description = "Runs of a target and what it depends on, newest first, each with the state of its steps. Requires `pipelines:read`. Paginated (`limit`/`offset`).",
    params(PageQuery),
    responses(
        (status = 200, body = pnex_core::Paginated<pnex_core::pipelines::Build>),
        (status = 403, description = "`access-denied`", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn builds(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Query(q): Query<PageQuery>,
) -> Result<Response> {
    let page = q.page();
    match data_builds::list(&ctx.db, org.org.id, page.limit, page.offset).await {
        Ok((count, rows)) => format::json(pagination::envelope(
            "/api/v1/data-builds",
            &[],
            page,
            count,
            rows,
        )),
        Err(e) => build_error(e),
    }
}

/// `GET /api/v1/data-builds/{id}`
#[utoipa::path(
    get,
    path = "/api/v1/data-builds/{id}",
    tag = "pipelines",
    operation_id = "data_builds_get",
    summary = "Get a chained run",
    description = "One chained run with the state of its steps. Requires `pipelines:read`.",
    params(("id" = String, Path, format = "uuid", description = "Build id (UUID)")),
    responses(
        (status = 200, body = pnex_core::pipelines::Build),
        (status = 403, description = "`access-denied`", body = crate::openapi::ErrorDetail),
        (status = 404, description = "`build-not-found`", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn build(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(id): Path<Uuid>,
) -> Result<Response> {
    match data_builds::get(&ctx.db, org.org.id, id).await {
        Ok(b) => format::json(b),
        Err(e) => build_error(e),
    }
}

#[derive(serde::Deserialize, utoipa::IntoParams)]
#[into_params(parameter_in = Query)]
struct PlanQuery {
    target_kind: BuildTargetKind,
    /// Pipeline or import id.
    #[param(format = "uuid", value_type = String)]
    target_id: Uuid,
}

/// `GET /api/v1/data-builds/plan`
#[utoipa::path(
    get,
    path = "/api/v1/data-builds/plan",
    tag = "pipelines",
    operation_id = "data_builds_plan",
    summary = "Show what a chained run would do",
    description = "The steps a run of this target would go through, dependencies first, without starting anything. Requires `pipelines:read`.",
    params(PlanQuery),
    responses(
        (status = 200, body = pnex_core::pipelines::BuildPlan),
        (status = 403, description = "`access-denied`", body = crate::openapi::ErrorDetail),
        (status = 404, description = "`build-not-found`", body = crate::openapi::ErrorDetail),
        (status = 409, description = "`build-cycle`, `build-dependency-missing` or `build-too-long`", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn build_plan(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Query(q): Query<PlanQuery>,
) -> Result<Response> {
    match data_builds::steps_of(&ctx.db, org.org.id, q.target_kind, q.target_id).await {
        Ok((_, steps)) => format::json(pnex_core::pipelines::BuildPlan { steps }),
        Err(e) => build_error(e),
    }
}

/// `POST /api/v1/data-builds`
#[utoipa::path(
    post,
    path = "/api/v1/data-builds",
    tag = "pipelines",
    operation_id = "data_builds_start",
    summary = "Run a target and what it depends on",
    description = "Runs the dependencies of a pipeline or an import in order, then the target. A failed step blocks what depends on it, which keeps its last valid version. During a run of the same target, asks for one more pass and returns the running one. Requires `pipelines:run`, plus `datasets:read` and `datasets:write` when a step is a pipeline and `ontology:write` when a step is an import.",
    request_body = pnex_core::pipelines::BuildInput,
    responses(
        (status = 202, body = pnex_core::pipelines::Build),
        (status = 400, description = "`field-invalid` on `target_id`", body = crate::openapi::ErrorDetail),
        (status = 403, description = "`access-denied`", body = crate::openapi::ErrorDetail),
        (status = 404, description = "`build-not-found`", body = crate::openapi::ErrorDetail),
        (status = 409, description = "`build-cycle`, `build-dependency-missing` or `build-too-long`", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn build_start(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Json(input): Json<BuildInput>,
) -> Result<Response> {
    let Ok(target) = input.target_id.parse::<Uuid>() else {
        return Ok(super::field_error(
            "target_id",
            err_codes::FIELD_INVALID_UUID,
        ));
    };
    let org_id = org.org.id;
    let kind = input.target_kind;
    let steps = match data_builds::steps_of(&ctx.db, org_id, kind, target).await {
        Ok((_, steps)) => steps,
        Err(e) => return build_error(e),
    };
    deny_without_rights(&org, &steps)?;
    match data_builds::request(
        &ctx.db,
        org_id,
        kind,
        target,
        "manual",
        Some(org.auth.user.id),
    )
    .await
    {
        Ok(b) => Ok((StatusCode::ACCEPTED, Json(b)).into_response()),
        Err(e) => build_error(e),
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

#[cfg(test)]
mod tests {
    use super::run_rows;
    use std::collections::BTreeMap;

    fn row(cells: &[(&str, &str)]) -> BTreeMap<String, String> {
        cells
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    #[test]
    fn rows_must_fit_the_asked_axis() {
        let doc: pnex_core::pipeline::Document = serde_json::from_value(serde_json::json!({
            "key": "site_rule", "name": "p",
            "steps": [{"enumerate": {"as": "start", "track": true,
                "cross": [{"as": "code", "asked": ["year"]}]}}],
        }))
        .unwrap();
        let axis = doc.asked_axis();
        assert!(axis.is_some());
        let err = |rows: &[BTreeMap<String, String>]| run_rows(axis, rows).err();
        assert_eq!(
            run_rows(axis, &[row(&[("code", "A1"), ("year", "2026")])]),
            Ok(())
        );
        assert_eq!(err(&[]), Some(("rows".into(), "required".into())));
        assert_eq!(
            err(&[row(&[("year", "2026")])]),
            Some(("rows.0.code".into(), "required".into()))
        );
        assert_eq!(
            err(&[row(&[("code", "A1"), ("other", "x")])]),
            Some(("rows.0.other".into(), "invalid".into()))
        );
        let long = "x".repeat(501);
        assert_eq!(
            err(&[row(&[("code", &long)])]),
            Some(("rows.0.code".into(), "max_length:500".into()))
        );
        assert_eq!(
            err(&vec![row(&[("code", "A1")]); 101]),
            Some(("rows".into(), "max_items:100".into()))
        );
        // A pipeline with no asked axis takes no rows.
        assert_eq!(run_rows(None, &[]), Ok(()));
        assert_eq!(
            run_rows(None, &[row(&[("code", "A1")])]).err(),
            Some(("rows".into(), "invalid".into()))
        );
    }
}
