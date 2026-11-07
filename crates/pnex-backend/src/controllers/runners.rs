//! Runners of the worker fabric (agent-environments.md lot E0). Org routes manage
//! them under `runners:read`, `runners:manage` and `runners:delete`; `/runner/...` routes are called
//! by the runner itself with its own token, and the org comes from that token (R1).

use axum::extract::DefaultBodyLimit;
use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::routing::{delete, get, post};
use axum::Json;
use loco_rs::controller::format;
use loco_rs::prelude::*;
use pnex_core::err_codes;
use pnex_core::runners::{
    capabilities_are_valid, spec_error, JoinTokenCreate, JoinTokenCreated, RunnerClaimedJob,
    RunnerJobCreate, RunnerJobFinish, RunnerJobKind, RunnerJobOutcome, RunnerJobSpec,
    RunnerJobState, RunnerJobView, RunnerRegister, RunnerRegistered, RunnerView, NAME_MAX,
};
use serde::Deserialize;
use uuid::Uuid;

use crate::auth::OrgContext;
use crate::controllers::pagination::PageParams;
use crate::services::environments;
use crate::services::runners::{self, RunnerError, RunnerPrincipal};

/// A scan report at its bounds (`pnex_core::environments`).
const FINISH_BODY_MAX: usize = 16 * 1024 * 1024;
const SBOM_BODY_MAX: usize = 64 * 1024 * 1024;
/// The bounds of `RunnerJobLogs`, with room for JSON escapes.
const LOGS_BODY_MAX: usize = 48 * 1024 * 1024;

pub fn routes() -> Routes {
    Routes::new()
        .prefix("/api/v1")
        .add("/runners", get(list))
        .add("/runners/join-tokens", post(create_join_token))
        .add("/runners/{id}", delete(remove))
        .add("/runner-jobs", get(list_jobs).post(create_job))
        .add("/runner-jobs/{id}", get(get_job))
        .add("/runner/register", post(register))
        .add("/runner/jobs/claim", post(claim))
        .add("/runner/jobs/{id}/heartbeat", post(heartbeat))
        .add(
            "/runner/jobs/{id}/finish",
            post(finish).layer(DefaultBodyLimit::max(FINISH_BODY_MAX)),
        )
        .add(
            "/runner/jobs/{id}/sbom",
            post(upload_sbom).layer(DefaultBodyLimit::max(SBOM_BODY_MAX)),
        )
        .add(
            "/runner/jobs/{id}/logs",
            post(upload_logs).layer(DefaultBodyLimit::max(LOGS_BODY_MAX)),
        )
}

#[derive(Debug, Deserialize, utoipa::IntoParams)]
#[into_params(parameter_in = Query)]
struct ListQuery {
    #[param(value_type = Option<i64>)]
    limit: Option<String>,
    #[param(value_type = Option<i64>)]
    offset: Option<String>,
}

fn runner_error(e: RunnerError) -> Error {
    match e {
        RunnerError::NotFound => super::coded_error(
            StatusCode::NOT_FOUND,
            err_codes::RUNNER_NOT_FOUND,
            "Runner or job not found",
            None,
        ),
        RunnerError::JoinTokenInvalid => super::coded_error(
            StatusCode::BAD_REQUEST,
            err_codes::RUNNER_JOIN_TOKEN_INVALID,
            "Invalid, used or expired join token",
            None,
        ),
        RunnerError::NotHeld => super::coded_error(
            StatusCode::CONFLICT,
            err_codes::RUNNER_JOB_NOT_HELD,
            "This runner does not hold the job any more",
            None,
        ),
        RunnerError::Db(e) => Error::DB(e),
    }
}

fn parse_id(id: &str) -> Result<Uuid> {
    Uuid::parse_str(id).map_err(|_| {
        super::coded_error(
            StatusCode::NOT_FOUND,
            err_codes::RUNNER_NOT_FOUND,
            "Runner or job not found",
            None,
        )
    })
}

/// The caller's runner, from `Authorization: Bearer <runner token>`.
async fn principal(ctx: &AppContext, headers: &HeaderMap) -> Result<RunnerPrincipal> {
    let token = headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "));
    let found = match token {
        Some(t) => runners::authenticate(&ctx.db, t).await.map_err(Error::DB)?,
        None => None,
    };
    found.ok_or_else(|| {
        super::coded_error(
            StatusCode::UNAUTHORIZED,
            err_codes::RUNNER_UNAUTHORIZED,
            "Unknown or revoked runner token",
            None,
        )
    })
}

#[utoipa::path(
    get,
    path = "/api/v1/runners",
    tag = "runners",
    operation_id = "runners_list",
    summary = "List runners",
    description = "Runners enrolled in the org, with what they announce and whether they were seen recently. Needs `runners:read`.",
    params(ListQuery),
    responses(
        (status = 200, body = pnex_core::Paginated<RunnerView>),
        (status = 403, description = "Missing permission (`access-denied`)", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn list(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Query(q): Query<ListQuery>,
) -> Result<Response> {
    let page = PageParams::from(q.limit.as_deref(), q.offset.as_deref());
    let (count, rows) =
        runners::list_runners(&ctx.db, org.org.id, page.limit as u64, page.offset as u64)
            .await
            .map_err(Error::DB)?;
    format::json(pnex_core::Paginated {
        count,
        next: page.next_offset(count).map(|o| o.to_string()),
        previous: page.previous_offset().map(|o| o.to_string()),
        results: rows,
    })
}

#[utoipa::path(
    post,
    path = "/api/v1/runners/join-tokens",
    tag = "runners",
    operation_id = "runners_join_tokens_create",
    summary = "Create a join token",
    description = "Creates a single-use token, valid one hour, that enrols one runner in the org. The token is shown once. Needs `runners:manage`.",
    request_body = JoinTokenCreate,
    responses(
        (status = 200, body = JoinTokenCreated),
        (status = 400, description = "Empty or too long `name`", body = crate::openapi::ErrorDetail),
        (status = 403, description = "Missing permission (`access-denied`)", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn create_join_token(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Json(body): Json<JoinTokenCreate>,
) -> Result<Response> {
    let name = body.name.trim();
    if name.is_empty() {
        return Ok(super::field_error("name", err_codes::FIELD_REQUIRED));
    }
    if name.chars().count() > NAME_MAX {
        return Ok(super::field_error(
            "name",
            &format!("max_length:{NAME_MAX}"),
        ));
    }
    let created = runners::create_join_token(&ctx.db, org.org.id, org.auth.user.id, name)
        .await
        .map_err(runner_error)?;
    format::json(created)
}

#[utoipa::path(
    delete,
    path = "/api/v1/runners/{id}",
    tag = "runners",
    operation_id = "runners_delete",
    summary = "Revoke a runner",
    description = "Revokes the runner's token and puts the jobs it held back in the queue. Needs `runners:delete`.",
    params(("id" = String, Path, format = "uuid", description = "Runner id")),
    responses(
        (status = 204, description = "Revoked"),
        (status = 403, description = "Missing permission (`access-denied`)", body = crate::openapi::ErrorDetail),
        (status = 404, description = "Not in this org (`runner-not-found`)", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn remove(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(id): Path<String>,
) -> Result<Response> {
    let id = parse_id(&id)?;
    runners::delete_runner(&ctx.db, org.org.id, id)
        .await
        .map_err(runner_error)?;
    Ok(StatusCode::NO_CONTENT.into_response())
}

#[utoipa::path(
    post,
    path = "/api/v1/runner-jobs",
    tag = "runners",
    operation_id = "runner_jobs_create",
    summary = "Queue a job",
    description = "Queues a job for a runner of the org whose capabilities cover the requirements. The image must be pinned by digest. If no runner takes it within an hour it expires, it is never moved elsewhere. Needs `runners:manage`.",
    request_body = RunnerJobCreate,
    responses(
        (status = 200, body = RunnerJobView),
        (status = 400, description = "Image not pinned by digest (`unpinned`), invalid command, timeout or capability", body = crate::openapi::ErrorDetail),
        (status = 403, description = "Missing permission (`access-denied`)", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn create_job(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Json(body): Json<RunnerJobCreate>,
) -> Result<Response> {
    if let Some((field, token)) = spec_error(&body.spec) {
        return Ok(super::field_error(field, token));
    }
    // A build carries credentials and has its own route, with its own permission.
    if body.kind != RunnerJobKind::Probe {
        return Ok(super::field_error("kind", err_codes::FIELD_INVALID));
    }
    if !capabilities_are_valid(&body.required_capabilities) {
        return Ok(super::field_error(
            "required_capabilities",
            err_codes::FIELD_INVALID,
        ));
    }
    let job = runners::create_job(&ctx.db, org.org.id, org.auth.user.id, &body)
        .await
        .map_err(Error::DB)?;
    format::json(job)
}

#[utoipa::path(
    get,
    path = "/api/v1/runner-jobs",
    tag = "runners",
    operation_id = "runner_jobs_list",
    summary = "List jobs",
    description = "Jobs of the org, newest first. Needs `runners:read`.",
    params(ListQuery),
    responses(
        (status = 200, body = pnex_core::Paginated<RunnerJobView>),
        (status = 403, description = "Missing permission (`access-denied`)", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn list_jobs(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Query(q): Query<ListQuery>,
) -> Result<Response> {
    let page = PageParams::from(q.limit.as_deref(), q.offset.as_deref());
    let (count, rows) =
        runners::list_jobs(&ctx.db, org.org.id, page.limit as u64, page.offset as u64)
            .await
            .map_err(Error::DB)?;
    format::json(pnex_core::Paginated {
        count,
        next: page.next_offset(count).map(|o| o.to_string()),
        previous: page.previous_offset().map(|o| o.to_string()),
        results: rows,
    })
}

#[utoipa::path(
    get,
    path = "/api/v1/runner-jobs/{id}",
    tag = "runners",
    operation_id = "runner_jobs_get",
    summary = "Get a job",
    description = "State and outcome of one job. Needs `runners:read`.",
    params(("id" = String, Path, format = "uuid", description = "Job id")),
    responses(
        (status = 200, body = RunnerJobView),
        (status = 403, description = "Missing permission (`access-denied`)", body = crate::openapi::ErrorDetail),
        (status = 404, description = "Not in this org (`runner-not-found`)", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn get_job(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(id): Path<String>,
) -> Result<Response> {
    let id = parse_id(&id)?;
    let job = runners::get_job(&ctx.db, org.org.id, id)
        .await
        .map_err(runner_error)?;
    format::json(job)
}

#[utoipa::path(
    post,
    path = "/api/v1/runner/register",
    tag = "runners",
    operation_id = "runner_register",
    summary = "Enrol a runner with a join token",
    description = "Called by `pnex-runner`, no user session: consumes a valid join token and returns the runner's own token, shown once. Rate limited per client address.",
    request_body = RunnerRegister,
    responses(
        (status = 200, body = RunnerRegistered),
        (status = 400, description = "Invalid, used or expired token (`runner-join-token-invalid`), or invalid capabilities", body = crate::openapi::ErrorDetail),
        (status = 429, description = "Too many attempts", body = crate::openapi::ErrorDetail)
    ),
    security(())
)]
async fn register(
    State(ctx): State<AppContext>,
    Json(body): Json<RunnerRegister>,
) -> Result<Response> {
    if !capabilities_are_valid(&body.capabilities) {
        return Ok(super::field_error("capabilities", err_codes::FIELD_INVALID));
    }
    let registered = runners::register(&ctx.db, &body.join_token, &body.capabilities)
        .await
        .map_err(runner_error)?;
    format::json(registered)
}

#[utoipa::path(
    post,
    path = "/api/v1/runner/jobs/claim",
    tag = "runners",
    operation_id = "runner_jobs_claim",
    summary = "Take the next job",
    description = "Called by a runner with its token: takes the oldest queued job of its org that its capabilities cover, or answers 204 when there is none.",
    responses(
        (status = 200, body = RunnerClaimedJob),
        (status = 204, description = "No job for this runner"),
        (status = 401, description = "Unknown or revoked token (`runner-unauthorized`)", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = []))
)]
async fn claim(State(ctx): State<AppContext>, headers: HeaderMap) -> Result<Response> {
    let runner = principal(&ctx, &headers).await?;
    let Some(mut job) = runners::claim(&ctx.db, runner).await.map_err(Error::DB)? else {
        return Ok(StatusCode::NO_CONTENT.into_response());
    };
    if job.kind == RunnerJobKind::EnvironmentBuild {
        let input = match crate::controllers::secrets::keyring(&ctx) {
            Ok(ring) => {
                let storage = crate::services::artifact_store::S3Config::from_config(&ctx.config);
                let size = crate::services::fabric_settings::build(&ctx.config);
                environments::build_input(&ctx.db, &ring, &storage, runner.org_id, &job.id)
                    .await
                    .map(|mut input| {
                        input.resources = Some(pnex_core::runners::BuildResources {
                            memory: size.memory,
                            ephemeral_storage: size.ephemeral_storage,
                        });
                        input
                    })
            }
            Err(_) => None,
        };
        // Nothing to run with: the job fails here, and never reaches a pod.
        let Some(input) = input else {
            let id = parse_id(&job.id)?;
            let failed = RunnerJobFinish {
                outcome: RunnerJobOutcome::Internal,
                exit_code: None,
                image_digest: None,
                scan_report: None,
            };
            runners::finish(&ctx.db, runner, id, &failed)
                .await
                .map_err(runner_error)?;
            return Ok(StatusCode::NO_CONTENT.into_response());
        };
        job.build = Some(input);
    }
    if job.kind == RunnerJobKind::Sandbox {
        let ring = crate::controllers::secrets::keyring(&ctx).ok();
        let input = crate::services::agent_runs::sandbox_input(
            &ctx.db,
            ring.as_ref(),
            runner.org_id,
            &job.id,
        )
        .await;
        // No run behind it, or no gateway on this deployment: an agent without its model
        // is not started.
        let Some(input) = input else {
            let id = parse_id(&job.id)?;
            let failed = RunnerJobFinish {
                outcome: RunnerJobOutcome::Internal,
                exit_code: None,
                image_digest: None,
                scan_report: None,
            };
            runners::finish(&ctx.db, runner, id, &failed)
                .await
                .map_err(runner_error)?;
            return Ok(StatusCode::NO_CONTENT.into_response());
        };
        job.sandbox = Some(input);
    }
    if job.kind == RunnerJobKind::EnvironmentScan {
        // Without a read login the scan goes on: the registry may be public, and a refused
        // pull ends the job as `image_pull` anyway.
        if let Ok(ring) = crate::controllers::secrets::keyring(&ctx) {
            job.scan = environments::scan_input(&ctx.db, &ring, runner.org_id, &job.id)
                .await
                .map_err(Error::DB)?;
        }
    }
    format::json(job)
}

#[utoipa::path(
    post,
    path = "/api/v1/runner/jobs/{id}/heartbeat",
    tag = "runners",
    operation_id = "runner_jobs_heartbeat",
    summary = "Renew the lease of a job",
    description = "Called by the runner holding the job. A 409 means it lost the job and must destroy its workload.",
    params(("id" = String, Path, format = "uuid", description = "Job id")),
    responses(
        (status = 204, description = "Lease renewed"),
        (status = 401, description = "Unknown or revoked token (`runner-unauthorized`)", body = crate::openapi::ErrorDetail),
        (status = 404, description = "Malformed id (`runner-not-found`)", body = crate::openapi::ErrorDetail),
        (status = 409, description = "Job not held by this runner (`runner-job-not-held`)", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = []))
)]
async fn heartbeat(
    State(ctx): State<AppContext>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Response> {
    let runner = principal(&ctx, &headers).await?;
    let id = parse_id(&id)?;
    runners::heartbeat(&ctx.db, runner, id)
        .await
        .map_err(runner_error)?;
    Ok(StatusCode::NO_CONTENT.into_response())
}

#[utoipa::path(
    post,
    path = "/api/v1/runner/jobs/{id}/finish",
    tag = "runners",
    operation_id = "runner_jobs_finish",
    summary = "Report how a job ended",
    description = "Called by the runner holding the job. Repeating the call after a lost answer changes nothing.",
    params(("id" = String, Path, format = "uuid", description = "Job id")),
    request_body = RunnerJobFinish,
    responses(
        (status = 204, description = "Recorded"),
        (status = 401, description = "Unknown or revoked token (`runner-unauthorized`)", body = crate::openapi::ErrorDetail),
        (status = 404, description = "Malformed id (`runner-not-found`)", body = crate::openapi::ErrorDetail),
        (status = 409, description = "Job not held by this runner (`runner-job-not-held`)", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = []))
)]
async fn finish(
    State(ctx): State<AppContext>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(mut body): Json<RunnerJobFinish>,
) -> Result<Response> {
    let runner = principal(&ctx, &headers).await?;
    let id = parse_id(&id)?;
    // While the job is still held: what a sandbox did not send is missing, and a required
    // output missing fails a run the agent ended well (D729).
    if runners::heartbeat(&ctx.db, runner, id).await.is_ok() {
        let closed =
            crate::services::agent_artifacts::close_outputs(&ctx.db, runner.org_id, id).await;
        match closed {
            Ok(Some(true)) if body.outcome == RunnerJobOutcome::Succeeded => {
                body.outcome = RunnerJobOutcome::Failed;
            }
            Ok(_) => {}
            Err(e) => tracing::warn!(error = %e, job = %id, "outputs not closed"),
        }
    }
    runners::finish(&ctx.db, runner, id, &body)
        .await
        .map_err(runner_error)?;
    // An ended run becomes a fact of the ontology; a job without a run is skipped there.
    crate::services::agent_ops::spawn_projection(&ctx, runner.org_id, id);
    if let (RunnerJobOutcome::Succeeded, Some(digest)) = (body.outcome, &body.image_digest) {
        environments::record_digest(&ctx.db, runner.org_id, id, digest)
            .await
            .map_err(Error::DB)?;
        let scanner = std::env::var("PNEX_IMAGE_SCANNER").ok();
        if let Err(e) =
            environments::start_scan(&ctx.db, runner.org_id, id, scanner.as_deref()).await
        {
            tracing::warn!(error = %e, "image scan not queued");
        }
    }
    if let (RunnerJobOutcome::Succeeded, Some(report)) = (body.outcome, &body.scan_report) {
        environments::record_scan(&ctx.db, runner.org_id, id, report)
            .await
            .map_err(Error::DB)?;
        crate::services::software_supply::spawn_projection(ctx.db.clone(), runner.org_id, id);
        let journal = ctx.clone();
        let org_id = runner.org_id;
        tokio::spawn(async move {
            if let Err(e) = crate::services::scan_journal::journal_scan(&journal, org_id, id).await
            {
                tracing::warn!(error = %e, "scan not journaled");
            }
        });
    }
    Ok(StatusCode::NO_CONTENT.into_response())
}

#[utoipa::path(
    post,
    path = "/api/v1/runner/jobs/{id}/sbom",
    tag = "runners",
    operation_id = "runner_jobs_sbom",
    summary = "Upload the SBOM of a scan",
    description = "Called by the runner holding this scan job, before finishing it, with the CycloneDX document as its body. Authenticated by the runner token.",
    params(("id" = String, Path, format = "uuid", description = "Job id")),
    request_body(content = String, content_type = "application/json"),
    responses(
        (status = 204, description = "Stored"),
        (status = 401, description = "`runner-unauthorized`", body = crate::openapi::ErrorDetail),
        (status = 409, description = "`runner-job-not-held`", body = crate::openapi::ErrorDetail),
        (status = 503, description = "`environment-sbom-unavailable`", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = []))
)]
async fn upload_sbom(
    State(ctx): State<AppContext>,
    headers: HeaderMap,
    Path(id): Path<String>,
    document: String,
) -> Result<Response> {
    let runner = principal(&ctx, &headers).await?;
    let id = parse_id(&id)?;
    // Only for the job this runner holds right now.
    runners::heartbeat(&ctx.db, runner, id)
        .await
        .map_err(runner_error)?;
    if let Err(e) =
        crate::services::scan_journal::store_sbom(&ctx, runner.org_id, id, &document).await
    {
        tracing::warn!(error = %e, "sbom upload failed");
        return Err(Error::CustomError(
            StatusCode::SERVICE_UNAVAILABLE,
            loco_rs::controller::ErrorDetail::new(
                err_codes::ENVIRONMENT_SBOM_UNAVAILABLE,
                "The SBOM could not be stored.",
            ),
        ));
    }
    Ok(StatusCode::NO_CONTENT.into_response())
}

#[utoipa::path(
    post,
    path = "/api/v1/runner/jobs/{id}/logs",
    tag = "runners",
    operation_id = "runner_jobs_logs",
    summary = "Upload the logs of a job",
    description = "Called by the runner holding the job, before finishing it, or within ten minutes after a person stopped it, with the last lines of each container, delivered secrets masked. The server masks credential-shaped words again and journals one record per line (D719). Authenticated by the runner token.",
    params(("id" = String, Path, format = "uuid", description = "Job id")),
    request_body = pnex_core::runners::RunnerJobLogs,
    responses(
        (status = 204, description = "Accepted"),
        (status = 400, description = "`field-invalid` (`errors.fields`)", body = crate::openapi::ErrorDetail),
        (status = 401, description = "`runner-unauthorized`", body = crate::openapi::ErrorDetail),
        (status = 409, description = "`runner-job-not-held`", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = []))
)]
async fn upload_logs(
    State(ctx): State<AppContext>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(logs): Json<pnex_core::runners::RunnerJobLogs>,
) -> Result<Response> {
    let runner = principal(&ctx, &headers).await?;
    let id = parse_id(&id)?;
    // The job this runner holds, or one a person stopped a moment ago (D759).
    runners::may_send_logs(&ctx.db, runner, id)
        .await
        .map_err(runner_error)?;
    if let Some((field, token)) = logs.violation() {
        return Ok(super::field_error(field, &token));
    }
    // Best effort, like the journal of a scan: the job's result never waits for its logs.
    let org_id = runner.org_id;
    tokio::spawn(async move {
        if let Err(e) = crate::services::job_logs::store(&ctx, org_id, id, &logs).await {
            tracing::warn!(error = %e, job = %id, "job logs not journaled");
        }
    });
    Ok(StatusCode::NO_CONTENT.into_response())
}

pub fn openapi() -> utoipa::openapi::OpenApi {
    #[derive(utoipa::OpenApi)]
    #[openapi(
        paths(
            upload_sbom,
            upload_logs,
            list,
            create_join_token,
            remove,
            create_job,
            list_jobs,
            get_job,
            register,
            claim,
            heartbeat,
            finish
        ),
        components(schemas(
            RunnerView,
            JoinTokenCreate,
            JoinTokenCreated,
            RunnerRegister,
            RunnerRegistered,
            RunnerJobCreate,
            RunnerJobSpec,
            RunnerJobKind,
            pnex_core::runners::RunnerBuildInput,
            pnex_core::runners::RunnerScanInput,
            pnex_core::environments::ScanComponent,
            pnex_core::runners::RunnerBuildCache,
            RunnerJobState,
            RunnerJobOutcome,
            RunnerJobView,
            RunnerClaimedJob,
            RunnerJobFinish,
            pnex_core::runners::RunnerJobLogs,
            pnex_core::runners::RunnerContainerLog
        ))
    )]
    struct Doc;
    <Doc as utoipa::OpenApi>::openapi()
}
