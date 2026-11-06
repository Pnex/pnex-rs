//! Agent environments API (agent-environments.md D702, D704). Org from the principal
//! (R1), permissions applied by `OrgContext` (R2).

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::Json;
use loco_rs::controller::{format, ErrorDetail};
use loco_rs::prelude::*;
use pnex_core::environments::{EnvironmentActivate, EnvironmentInput, EnvironmentVersionInput};
use pnex_core::err_codes;
use uuid::Uuid;

use crate::auth::OrgContext;
use crate::controllers::pagination::{self, PageParams};
use crate::services::environments::{self, EnvironmentError};
use crate::services::scan_journal;

pub fn routes() -> Routes {
    Routes::new()
        .prefix("/api/v1")
        .add("/environments", get(list).post(create))
        .add("/environments/{id}", get(one).put(update).delete(remove))
        .add(
            "/environments/{id}/versions",
            get(versions).post(create_version),
        )
        .add("/environments/{id}/versions/{number}", get(version))
        .add(
            "/environments/{id}/versions/{number}/builds",
            post(start_build),
        )
        .add("/environments/{id}/builds", get(builds))
        .add("/environments/{id}/builds/{build}", get(one_build))
        .add("/environments/{id}/builds/{build}/scan", get(scan))
        .add("/environments/{id}/builds/{build}/logs", get(build_logs))
        .add(
            "/environments/scan-policy",
            get(get_scan_policy).put(put_scan_policy),
        )
        .add("/environments/{id}/builds/{build}/sbom", get(sbom))
        .add("/environments/{id}/scans", get(scan_history))
        .add("/environments/{id}/builds/{build}/activate", post(activate))
}

pub fn openapi() -> utoipa::openapi::OpenApi {
    #[derive(utoipa::OpenApi)]
    #[openapi(paths(
        get_scan_policy,
        put_scan_policy,
        build_logs,
        list,
        one,
        create,
        update,
        remove,
        versions,
        create_version,
        version,
        start_build,
        builds,
        one_build,
        scan,
        sbom,
        scan_history,
        activate
    ))]
    struct Doc;
    <Doc as utoipa::OpenApi>::openapi()
}

fn environment_error(e: EnvironmentError) -> Result<Response> {
    Err(match e {
        EnvironmentError::Invalid { field, token } => {
            return Ok(super::field_error(&field, &token))
        }
        EnvironmentError::NotFound => Error::CustomError(
            StatusCode::NOT_FOUND,
            ErrorDetail::new(err_codes::ENVIRONMENT_NOT_FOUND, "Environment not found."),
        ),
        EnvironmentError::VersionMissing => Error::CustomError(
            StatusCode::NOT_FOUND,
            ErrorDetail::new(
                err_codes::ENVIRONMENT_VERSION_NOT_FOUND,
                "This environment has no such version.",
            ),
        ),
        EnvironmentError::NameTaken => Error::CustomError(
            StatusCode::CONFLICT,
            ErrorDetail::new(
                err_codes::ENVIRONMENT_NAME_TAKEN,
                "An environment of the org already has this name.",
            ),
        ),
        EnvironmentError::VersionConflict { current } => super::coded_error(
            StatusCode::CONFLICT,
            err_codes::ENVIRONMENT_VERSION_CONFLICT,
            "The environment changed since it was read; reload it.",
            Some(serde_json::json!({ "current": current })),
        ),
        EnvironmentError::SecretInSource { at } => super::coded_error(
            StatusCode::BAD_REQUEST,
            err_codes::ENVIRONMENT_SECRET_IN_DOCKERFILE,
            "The source holds what looks like a credential; use a secret instead.",
            Some(serde_json::json!({ "at": at })),
        ),
        EnvironmentError::BuildMissing => Error::CustomError(
            StatusCode::NOT_FOUND,
            ErrorDetail::new(err_codes::ENVIRONMENT_BUILD_NOT_FOUND, "Build not found."),
        ),
        EnvironmentError::BuildUnavailable => Error::CustomError(
            StatusCode::CONFLICT,
            ErrorDetail::new(
                err_codes::ENVIRONMENT_BUILD_UNAVAILABLE,
                "This deployment has no builder image.",
            ),
        ),
        EnvironmentError::NoWriteSecret => Error::CustomError(
            StatusCode::CONFLICT,
            ErrorDetail::new(
                err_codes::ENVIRONMENT_BUILD_NO_WRITE_SECRET,
                "The registry of this environment has no write secret.",
            ),
        ),
        EnvironmentError::BuildRunning => Error::CustomError(
            StatusCode::CONFLICT,
            ErrorDetail::new(
                err_codes::ENVIRONMENT_BUILD_RUNNING,
                "A build of this environment is not finished.",
            ),
        ),
        EnvironmentError::BuildNotReady => Error::CustomError(
            StatusCode::CONFLICT,
            ErrorDetail::new(
                err_codes::ENVIRONMENT_BUILD_NOT_READY,
                "Only a build that produced an image can be activated.",
            ),
        ),
        EnvironmentError::ScanRefused { breaches } => super::coded_error(
            StatusCode::CONFLICT,
            err_codes::ENVIRONMENT_SCAN_REFUSED,
            "The image is over the scan threshold of the org: rebuild it.",
            Some(serde_json::json!({ "breaches": breaches.to_string() })),
        ),
        EnvironmentError::ScanMissing => Error::CustomError(
            StatusCode::NOT_FOUND,
            ErrorDetail::new(
                err_codes::ENVIRONMENT_SCAN_NOT_FOUND,
                "This build has no scan.",
            ),
        ),
        EnvironmentError::Db(e) => {
            tracing::error!(error = %e, "environment database error");
            Error::InternalServerError
        }
    })
}

#[derive(serde::Deserialize, utoipa::IntoParams)]
#[into_params(parameter_in = Query)]
struct PageQuery {
    #[param(value_type = Option<i64>)]
    limit: Option<String>,
    #[param(value_type = Option<i64>)]
    offset: Option<String>,
}

/// `GET /api/v1/environments`
#[utoipa::path(
    get,
    path = "/api/v1/environments",
    tag = "environments",
    operation_id = "environments_list",
    summary = "List the environments",
    description = "Environments agents can run in. Requires `environments:read`. Paginated (`limit`/`offset`).",
    params(PageQuery),
    responses(
        (status = 200, body = pnex_core::Paginated<pnex_core::environments::Environment>),
        (status = 403, description = "`access-denied`", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn list(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Query(q): Query<PageQuery>,
) -> Result<Response> {
    let page = PageParams::from(q.limit.as_deref(), q.offset.as_deref());
    match environments::list(&ctx.db, org.org.id, page.limit, page.offset).await {
        Ok((count, rows)) => format::json(pagination::envelope(
            "/api/v1/environments",
            &[],
            page,
            count,
            rows,
        )),
        Err(e) => environment_error(e),
    }
}

/// `POST /api/v1/environments`
#[utoipa::path(
    post,
    path = "/api/v1/environments",
    tag = "environments",
    operation_id = "environments_create",
    summary = "Create an environment",
    description = "Creates an environment and names the image registry of the org its images go to. Its source comes with its versions. Requires `environments:write`.",
    request_body = pnex_core::environments::EnvironmentInput,
    responses(
        (status = 201, body = pnex_core::environments::Environment),
        (status = 400, description = "`field-invalid` (`errors.fields`)", body = crate::openapi::ErrorDetail),
        (status = 403, description = "`access-denied`", body = crate::openapi::ErrorDetail),
        (status = 409, description = "`environment-name-taken`", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn create(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Json(input): Json<EnvironmentInput>,
) -> Result<Response> {
    match environments::create(&ctx.db, org.org.id, org.auth.user.id, &input).await {
        Ok(e) => Ok((StatusCode::CREATED, Json(e)).into_response()),
        Err(e) => environment_error(e),
    }
}

/// `GET /api/v1/environments/{id}`
#[utoipa::path(
    get,
    path = "/api/v1/environments/{id}",
    tag = "environments",
    operation_id = "environments_get",
    summary = "Get an environment",
    description = "One environment. Requires `environments:read`.",
    params(("id" = String, Path, format = "uuid", description = "Environment id (UUID)")),
    responses(
        (status = 200, body = pnex_core::environments::Environment),
        (status = 403, description = "`access-denied`", body = crate::openapi::ErrorDetail),
        (status = 404, description = "`environment-not-found`", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn one(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(id): Path<Uuid>,
) -> Result<Response> {
    match environments::get(&ctx.db, org.org.id, id).await {
        Ok(e) => format::json(e),
        Err(e) => environment_error(e),
    }
}

/// `PUT /api/v1/environments/{id}`
#[utoipa::path(
    put,
    path = "/api/v1/environments/{id}",
    tag = "environments",
    operation_id = "environments_update",
    summary = "Replace an environment",
    description = "Replaces its name, description and registry, on the version that was read (`expected_version`). Requires `environments:write`.",
    params(("id" = String, Path, format = "uuid", description = "Environment id (UUID)")),
    request_body = pnex_core::environments::EnvironmentInput,
    responses(
        (status = 200, body = pnex_core::environments::Environment),
        (status = 400, description = "`field-invalid` (`errors.fields`)", body = crate::openapi::ErrorDetail),
        (status = 403, description = "`access-denied`", body = crate::openapi::ErrorDetail),
        (status = 404, description = "`environment-not-found`", body = crate::openapi::ErrorDetail),
        (status = 409, description = "`environment-name-taken` or `environment-version-conflict` (args `current`)", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn update(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(id): Path<Uuid>,
    Json(input): Json<EnvironmentInput>,
) -> Result<Response> {
    match environments::update(&ctx.db, org.org.id, id, &input).await {
        Ok(e) => format::json(e),
        Err(e) => environment_error(e),
    }
}

/// `DELETE /api/v1/environments/{id}`
#[utoipa::path(
    delete,
    path = "/api/v1/environments/{id}",
    tag = "environments",
    operation_id = "environments_delete",
    summary = "Delete an environment",
    description = "Removes the environment and its versions. Images stay in the registry. Requires `environments:delete`.",
    params(("id" = String, Path, format = "uuid", description = "Environment id (UUID)")),
    responses(
        (status = 204, description = "Deleted"),
        (status = 403, description = "`access-denied`", body = crate::openapi::ErrorDetail),
        (status = 404, description = "`environment-not-found`", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn remove(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(id): Path<Uuid>,
) -> Result<Response> {
    match environments::delete(&ctx.db, org.org.id, id).await {
        Ok(()) => Ok(StatusCode::NO_CONTENT.into_response()),
        Err(e) => environment_error(e),
    }
}

/// `GET /api/v1/environments/{id}/versions`
#[utoipa::path(
    get,
    path = "/api/v1/environments/{id}/versions",
    tag = "environments",
    operation_id = "environments_versions_list",
    summary = "List the versions of an environment",
    description = "Newest first, without their source. Requires `environments:read`. Paginated (`limit`/`offset`).",
    params(
        ("id" = String, Path, format = "uuid", description = "Environment id (UUID)"),
        PageQuery
    ),
    responses(
        (status = 200, body = pnex_core::Paginated<pnex_core::environments::EnvironmentVersionSummary>),
        (status = 403, description = "`access-denied`", body = crate::openapi::ErrorDetail),
        (status = 404, description = "`environment-not-found`", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn versions(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(id): Path<Uuid>,
    Query(q): Query<PageQuery>,
) -> Result<Response> {
    let page = PageParams::from(q.limit.as_deref(), q.offset.as_deref());
    match environments::list_versions(&ctx.db, org.org.id, id, page.limit, page.offset).await {
        Ok((count, rows)) => format::json(pagination::envelope(
            &format!("/api/v1/environments/{id}/versions"),
            &[],
            page,
            count,
            rows,
        )),
        Err(e) => environment_error(e),
    }
}

/// `POST /api/v1/environments/{id}/versions`
#[utoipa::path(
    post,
    path = "/api/v1/environments/{id}/versions",
    tag = "environments",
    operation_id = "environments_versions_create",
    summary = "Save a new version of an environment",
    description = "Saves a Dockerfile, its text context files and its build arguments as a new immutable version, on the latest one that was read (`expected_latest`, absent for the first). At most 1 MB in all. A source holding what looks like a credential is refused. Requires `environments:write`.",
    params(("id" = String, Path, format = "uuid", description = "Environment id (UUID)")),
    request_body = pnex_core::environments::EnvironmentVersionInput,
    responses(
        (status = 201, body = pnex_core::environments::EnvironmentVersionSummary),
        (status = 400, description = "`field-invalid` (`errors.fields`) or `environment-secret-in-dockerfile` (args `at`)", body = crate::openapi::ErrorDetail),
        (status = 403, description = "`access-denied`", body = crate::openapi::ErrorDetail),
        (status = 404, description = "`environment-not-found`", body = crate::openapi::ErrorDetail),
        (status = 409, description = "`environment-version-conflict` (args `current`)", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn create_version(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(id): Path<Uuid>,
    Json(input): Json<EnvironmentVersionInput>,
) -> Result<Response> {
    match environments::create_version(&ctx.db, org.org.id, org.auth.user.id, id, &input).await {
        Ok(v) => Ok((StatusCode::CREATED, Json(v)).into_response()),
        Err(e) => environment_error(e),
    }
}

/// `GET /api/v1/environments/{id}/versions/{number}`
#[utoipa::path(
    get,
    path = "/api/v1/environments/{id}/versions/{number}",
    tag = "environments",
    operation_id = "environments_versions_get",
    summary = "Get a version of an environment",
    description = "One version with its source. Requires `environments:read`.",
    params(
        ("id" = String, Path, format = "uuid", description = "Environment id (UUID)"),
        ("number" = i32, Path, description = "Version number")
    ),
    responses(
        (status = 200, body = pnex_core::environments::EnvironmentVersion),
        (status = 403, description = "`access-denied`", body = crate::openapi::ErrorDetail),
        (status = 404, description = "`environment-not-found` or `environment-version-not-found`", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn version(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path((id, number)): Path<(Uuid, i32)>,
) -> Result<Response> {
    match environments::get_version(&ctx.db, org.org.id, id, number).await {
        Ok(v) => format::json(v),
        Err(e) => environment_error(e),
    }
}

/// `POST /api/v1/environments/{id}/versions/{number}/builds`
#[utoipa::path(
    post,
    path = "/api/v1/environments/{id}/versions/{number}/builds",
    tag = "environments",
    operation_id = "environments_builds_create",
    summary = "Build a version of an environment",
    description = "Queues the build of this version on a runner that can build. Its image goes to the registry of the environment, which needs a write secret. The Dockerfile runs on that runner's cluster. One build at a time per environment. Requires `environments:build`.",
    params(
        ("id" = String, Path, format = "uuid", description = "Environment id (UUID)"),
        ("number" = i32, Path, description = "Version number")
    ),
    responses(
        (status = 201, body = pnex_core::environments::EnvironmentBuild),
        (status = 403, description = "`access-denied`", body = crate::openapi::ErrorDetail),
        (status = 404, description = "`environment-not-found` or `environment-version-not-found`", body = crate::openapi::ErrorDetail),
        (status = 409, description = "`environment-build-unavailable`, `environment-build-no-write-secret` or `environment-build-running`", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn start_build(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path((id, number)): Path<(Uuid, i32)>,
) -> Result<Response> {
    let builder = std::env::var("PNEX_IMAGE_BUILDER").ok();
    match environments::start_build(
        &ctx.db,
        org.org.id,
        org.auth.user.id,
        id,
        number,
        builder.as_deref(),
    )
    .await
    {
        Ok(b) => Ok((StatusCode::CREATED, Json(b)).into_response()),
        Err(e) => environment_error(e),
    }
}

/// `GET /api/v1/environments/{id}/builds`
#[utoipa::path(
    get,
    path = "/api/v1/environments/{id}/builds",
    tag = "environments",
    operation_id = "environments_builds_list",
    summary = "List the builds of an environment",
    description = "Newest first. Requires `environments:read`. Paginated (`limit`/`offset`).",
    params(
        ("id" = String, Path, format = "uuid", description = "Environment id (UUID)"),
        PageQuery
    ),
    responses(
        (status = 200, body = pnex_core::Paginated<pnex_core::environments::EnvironmentBuild>),
        (status = 403, description = "`access-denied`", body = crate::openapi::ErrorDetail),
        (status = 404, description = "`environment-not-found`", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn builds(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(id): Path<Uuid>,
    Query(q): Query<PageQuery>,
) -> Result<Response> {
    let page = PageParams::from(q.limit.as_deref(), q.offset.as_deref());
    match environments::list_builds(&ctx.db, org.org.id, id, page.limit, page.offset).await {
        Ok((count, rows)) => format::json(pagination::envelope(
            &format!("/api/v1/environments/{id}/builds"),
            &[],
            page,
            count,
            rows,
        )),
        Err(e) => environment_error(e),
    }
}

/// `GET /api/v1/environments/{id}/builds/{build}`
#[utoipa::path(
    get,
    path = "/api/v1/environments/{id}/builds/{build}",
    tag = "environments",
    operation_id = "environments_builds_get",
    summary = "Get a build of an environment",
    description = "Its state and, once ready, its image pinned by digest. Requires `environments:read`.",
    params(
        ("id" = String, Path, format = "uuid", description = "Environment id (UUID)"),
        ("build" = String, Path, format = "uuid", description = "Build id (UUID)")
    ),
    responses(
        (status = 200, body = pnex_core::environments::EnvironmentBuild),
        (status = 403, description = "`access-denied`", body = crate::openapi::ErrorDetail),
        (status = 404, description = "`environment-build-not-found`", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn one_build(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path((id, build)): Path<(Uuid, Uuid)>,
) -> Result<Response> {
    match environments::get_build(&ctx.db, org.org.id, id, build).await {
        Ok(b) => format::json(b),
        Err(e) => environment_error(e),
    }
}

/// `GET /api/v1/environments/scan-policy`
#[utoipa::path(
    get,
    path = "/api/v1/environments/scan-policy",
    tag = "environments",
    operation_id = "environments_scan_policy_get",
    summary = "Read the scan policy of the org",
    description = "The lowest severity of a fixable flaw that counts against an image (`critical` by default), and what such an image gets: a signal (default) or a refusal of its activation and its runs (D748). Requires `environments:read`.",
    responses(
        (status = 200, body = pnex_core::environments::ScanPolicy),
        (status = 403, description = "`access-denied`", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn get_scan_policy(State(ctx): State<AppContext>, org: OrgContext) -> Result<Response> {
    format::json(
        environments::scan_policy(&ctx.db, org.org.id)
            .await
            .map_err(Error::DB)?,
    )
}

/// `PUT /api/v1/environments/scan-policy`
#[utoipa::path(
    put,
    path = "/api/v1/environments/scan-policy",
    tag = "environments",
    operation_id = "environments_scan_policy_set",
    summary = "Set the scan policy of the org",
    description = "Applies at once to every check that follows: activation of a build and start of a run. Requires `environments:build` (Admin).",
    request_body = pnex_core::environments::ScanPolicy,
    responses(
        (status = 200, body = pnex_core::environments::ScanPolicy),
        (status = 403, description = "`access-denied`", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn put_scan_policy(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Json(body): Json<pnex_core::environments::ScanPolicy>,
) -> Result<Response> {
    format::json(
        environments::set_scan_policy(&ctx.db, org.org.id, org.auth.user.id, body)
            .await
            .map_err(Error::DB)?,
    )
}

/// `GET /api/v1/environments/{id}/builds/{build}/logs`
#[utoipa::path(
    get,
    path = "/api/v1/environments/{id}/builds/{build}/logs",
    tag = "environments",
    operation_id = "environments_builds_logs",
    summary = "Read the journal of a build",
    description = "The last lines of the build pod, oldest first, sent by the runner when the build ended, secrets masked (D733, D759). Requires `environments:read`. Paginated (`limit`/`offset`).",
    params(
        ("id" = String, Path, format = "uuid", description = "Environment id (UUID)"),
        ("build" = String, Path, format = "uuid", description = "Build id (UUID)"),
        PageQuery
    ),
    responses(
        (status = 200, body = pnex_core::Paginated<pnex_core::runners::RunnerJobLogLine>),
        (status = 403, description = "`access-denied`", body = crate::openapi::ErrorDetail),
        (status = 404, description = "`environment-build-not-found`", body = crate::openapi::ErrorDetail),
        (status = 503, description = "`environment-build-logs-unavailable`", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn build_logs(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path((id, build)): Path<(Uuid, Uuid)>,
    Query(q): Query<PageQuery>,
) -> Result<Response> {
    let b = match environments::get_build(&ctx.db, org.org.id, id, build).await {
        Ok(b) => b,
        Err(e) => return environment_error(e),
    };
    let job = Uuid::parse_str(&b.job_id).map_err(|_| Error::InternalServerError)?;
    let since_us = chrono::DateTime::parse_from_rfc3339(&b.created_at)
        .map(|t| t.timestamp_micros())
        .unwrap_or(0);
    let page = PageParams::from(q.limit.as_deref(), q.offset.as_deref());
    let found =
        crate::services::job_logs::lines(&ctx, org.org.id, job, since_us, page.limit, page.offset)
            .await;
    let (count, rows) = match found {
        Ok(Some(found)) => found,
        not_read => {
            if let Err(e) = not_read {
                tracing::warn!(error = %e, "build journal not read");
            }
            return Err(super::coded_error(
                StatusCode::SERVICE_UNAVAILABLE,
                err_codes::ENVIRONMENT_BUILD_LOGS_UNAVAILABLE,
                "The journal of builds is not available.",
                None,
            ));
        }
    };
    format::json(pagination::envelope(
        &format!("/api/v1/environments/{id}/builds/{build}/logs"),
        &[],
        page,
        count,
        rows,
    ))
}

/// `GET /api/v1/environments/{id}/builds/{build}/scan`
#[utoipa::path(
    get,
    path = "/api/v1/environments/{id}/builds/{build}/scan",
    tag = "environments",
    operation_id = "environments_builds_scan",
    summary = "Get the scan of a build",
    description = "The known vulnerabilities of the image of this build, from its latest scan, with the version that fixes each one when there is one. Requires `environments:read`.",
    params(
        ("id" = String, Path, format = "uuid", description = "Environment id (UUID)"),
        ("build" = String, Path, format = "uuid", description = "Build id (UUID)")
    ),
    responses(
        (status = 200, body = pnex_core::environments::EnvironmentScan),
        (status = 403, description = "`access-denied`", body = crate::openapi::ErrorDetail),
        (status = 404, description = "`environment-build-not-found` or `environment-scan-not-found`", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn scan(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path((id, build)): Path<(Uuid, Uuid)>,
) -> Result<Response> {
    match environments::get_scan(&ctx.db, org.org.id, id, build).await {
        Ok(s) => format::json(s),
        Err(e) => environment_error(e),
    }
}

/// `POST /api/v1/environments/{id}/builds/{build}/activate`
#[utoipa::path(
    post,
    path = "/api/v1/environments/{id}/builds/{build}/activate",
    tag = "environments",
    operation_id = "environments_builds_activate",
    summary = "Activate a build of an environment",
    description = "Makes this ready build the one the runs of the environment execute, on the version that was read (`expected_version`). Going back is activating an earlier build. Requires `environments:build`.",
    params(
        ("id" = String, Path, format = "uuid", description = "Environment id (UUID)"),
        ("build" = String, Path, format = "uuid", description = "Build id (UUID)")
    ),
    request_body = pnex_core::environments::EnvironmentActivate,
    responses(
        (status = 200, body = pnex_core::environments::Environment),
        (status = 403, description = "`access-denied`", body = crate::openapi::ErrorDetail),
        (status = 404, description = "`environment-not-found` or `environment-build-not-found`", body = crate::openapi::ErrorDetail),
        (status = 409, description = "`environment-build-not-ready` or `environment-version-conflict` (args `current`)", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn activate(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path((id, build)): Path<(Uuid, Uuid)>,
    Json(input): Json<EnvironmentActivate>,
) -> Result<Response> {
    match environments::activate(&ctx.db, org.org.id, id, build, input.expected_version).await {
        Ok(e) => format::json(e),
        Err(e) => environment_error(e),
    }
}

/// `GET /api/v1/environments/{id}/builds/{build}/sbom`
#[utoipa::path(
    get,
    path = "/api/v1/environments/{id}/builds/{build}/sbom",
    tag = "environments",
    operation_id = "environments_builds_sbom",
    summary = "Download the SBOM of a build",
    description = "The CycloneDX document from its latest scan, as an attachment. Requires `environments:read`.",
    params(
        ("id" = String, Path, format = "uuid", description = "Environment id (UUID)"),
        ("build" = String, Path, format = "uuid", description = "Build id (UUID)")
    ),
    responses(
        (status = 200, description = "CycloneDX JSON document", content_type = "application/json", body = String),
        (status = 403, description = "`access-denied`", body = crate::openapi::ErrorDetail),
        (status = 404, description = "`environment-build-not-found` or `environment-scan-not-found`", body = crate::openapi::ErrorDetail),
        (status = 503, description = "`environment-sbom-unavailable`", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn sbom(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path((id, build)): Path<(Uuid, Uuid)>,
) -> Result<Response> {
    if let Err(e) = environments::get_build(&ctx.db, org.org.id, id, build).await {
        return environment_error(e);
    }
    match scan_journal::read_sbom(&ctx, org.org.id, build).await {
        Ok(document) => Ok((
            [
                (axum::http::header::CONTENT_TYPE, "application/json"),
                (
                    axum::http::header::CONTENT_DISPOSITION,
                    "attachment; filename=\"sbom.cdx.json\"",
                ),
                (axum::http::header::X_CONTENT_TYPE_OPTIONS, "nosniff"),
            ],
            document,
        )
            .into_response()),
        Err(scan_journal::SbomError::Missing) => environment_error(EnvironmentError::ScanMissing),
        Err(scan_journal::SbomError::Unavailable(e)) => {
            tracing::warn!(error = %e, "sbom read failed");
            Err(Error::CustomError(
                StatusCode::SERVICE_UNAVAILABLE,
                ErrorDetail::new(
                    err_codes::ENVIRONMENT_SBOM_UNAVAILABLE,
                    "The SBOM cannot be read right now.",
                ),
            ))
        }
    }
}

/// `GET /api/v1/environments/{id}/scans`
#[utoipa::path(
    get,
    path = "/api/v1/environments/{id}/scans",
    tag = "environments",
    operation_id = "environments_scan_history",
    summary = "Follow the scans of an environment",
    description = "Each scan of the last 90 days with its counts per severity, newest first, to follow its exposure over time. Requires `environments:read`. Paginated (`limit`/`offset`).",
    params(
        ("id" = String, Path, format = "uuid", description = "Environment id (UUID)"),
        PageQuery
    ),
    responses(
        (status = 200, body = pnex_core::Paginated<pnex_core::environments::ScanHistoryEntry>),
        (status = 403, description = "`access-denied`", body = crate::openapi::ErrorDetail),
        (status = 404, description = "`environment-not-found`", body = crate::openapi::ErrorDetail),
        (status = 503, description = "`events-unavailable`", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn scan_history(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(id): Path<Uuid>,
    Query(q): Query<PageQuery>,
) -> Result<Response> {
    if let Err(e) = environments::get(&ctx.db, org.org.id, id).await {
        return environment_error(e);
    }
    let page = PageParams::from(q.limit.as_deref(), q.offset.as_deref());
    let unavailable = || {
        Error::CustomError(
            StatusCode::SERVICE_UNAVAILABLE,
            ErrorDetail::new(
                err_codes::EVENTS_UNAVAILABLE,
                "Scan events are unavailable.",
            ),
        )
    };
    match scan_journal::history(&ctx, org.org.id, id, page.limit, page.offset).await {
        Ok(Some((count, rows))) => format::json(pagination::envelope(
            &format!("/api/v1/environments/{id}/scans"),
            &[],
            page,
            count,
            rows,
        )),
        Ok(None) => Err(unavailable()),
        Err(e) => {
            tracing::warn!(error = %e, "scan history failed");
            Err(unavailable())
        }
    }
}
