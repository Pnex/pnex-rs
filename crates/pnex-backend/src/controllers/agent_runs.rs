//! Runs of hosted agents and their LLM gateway (agent-environments.md D731, D732). The run
//! routes take the org from the principal (R1) with permissions applied by `OrgContext`
//! (R2); the gateway routes take a run token, valid only while its job runs.

use axum::body::Bytes;
use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::routing::{get, post};
use axum::Json;
use loco_rs::controller::format;
use loco_rs::prelude::*;
use pnex_core::agent_runs::{AgentRunCreate, GATEWAY_BODY_MAX};
use pnex_core::err_codes;
use uuid::Uuid;

use crate::auth::OrgContext;
use crate::controllers::pagination::{self, PageParams};
use crate::services::agent_gateway::{self, GatewayError, Protocol};
use crate::services::agent_runs::{self, AgentRunError, RunPrincipal};

pub fn routes() -> Routes {
    Routes::new()
        .prefix("/api/v1")
        .add("/agent-runs", get(list).post(create))
        .add("/agent-runs/{id}", get(one))
        .add("/agent-runs/{id}/stop", post(stop))
        .add("/agent-runs/{id}/logs", get(logs))
        .add(
            "/agent-gateway/v1/messages",
            post(gateway_anthropic).layer(axum::extract::DefaultBodyLimit::max(GATEWAY_BODY_MAX)),
        )
        .add(
            "/agent-gateway/v1/chat/completions",
            post(gateway_openai).layer(axum::extract::DefaultBodyLimit::max(GATEWAY_BODY_MAX)),
        )
}

pub fn openapi() -> utoipa::openapi::OpenApi {
    #[derive(utoipa::OpenApi)]
    #[openapi(
        paths(list, create, one, stop, logs, gateway_anthropic, gateway_openai),
        components(schemas(pnex_core::runners::RunnerJobLogLine))
    )]
    struct Doc;
    <Doc as utoipa::OpenApi>::openapi()
}

fn run_error(e: AgentRunError) -> Result<Response> {
    Err(match e {
        AgentRunError::Invalid { field, token } => return Ok(super::field_error(&field, &token)),
        AgentRunError::NotFound => super::coded_error(
            StatusCode::NOT_FOUND,
            err_codes::AGENT_RUN_NOT_FOUND,
            "Run not found.",
            None,
        ),
        AgentRunError::EnvironmentNotFound => super::coded_error(
            StatusCode::NOT_FOUND,
            err_codes::ENVIRONMENT_NOT_FOUND,
            "Environment not found.",
            None,
        ),
        AgentRunError::NoActiveBuild => super::coded_error(
            StatusCode::CONFLICT,
            err_codes::AGENT_RUN_NO_ACTIVE_BUILD,
            "The environment has no active build to run.",
            None,
        ),
        AgentRunError::Ended => super::coded_error(
            StatusCode::CONFLICT,
            err_codes::AGENT_RUN_ENDED,
            "The run has already ended.",
            None,
        ),
        AgentRunError::Db(e) => {
            tracing::error!(error = %e, "agent run database error");
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

/// `GET /api/v1/agent-runs`
#[utoipa::path(
    get,
    path = "/api/v1/agent-runs",
    tag = "agent-runs",
    operation_id = "agent_runs_list",
    summary = "List the runs of hosted agents",
    description = "Newest first, with their state and the LLM usage counted by the gateway. Requires `agents:read`. Paginated (`limit`/`offset`).",
    params(PageQuery),
    responses(
        (status = 200, body = pnex_core::Paginated<pnex_core::agent_runs::AgentRun>),
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
    match agent_runs::list(&ctx.db, org.org.id, page.limit, page.offset).await {
        Ok((count, rows)) => format::json(pagination::envelope(
            "/api/v1/agent-runs",
            &[],
            page,
            count,
            rows,
        )),
        Err(e) => run_error(e.into()),
    }
}

/// `POST /api/v1/agent-runs`
#[utoipa::path(
    post,
    path = "/api/v1/agent-runs",
    tag = "agent-runs",
    operation_id = "agent_runs_create",
    summary = "Start a run",
    description = "Queues a run of the active build of an environment, in a sandbox of a runner of the org. The run reaches the org's LLM through the gateway, never with a provider key. Requires `agents:run`.",
    request_body = AgentRunCreate,
    responses(
        (status = 201, body = pnex_core::agent_runs::AgentRun),
        (status = 400, description = "`field-invalid` (`errors.fields`)", body = crate::openapi::ErrorDetail),
        (status = 403, description = "`access-denied`", body = crate::openapi::ErrorDetail),
        (status = 404, description = "`environment-not-found`", body = crate::openapi::ErrorDetail),
        (status = 409, description = "`agent-run-no-active-build`", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn create(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Json(input): Json<AgentRunCreate>,
) -> Result<Response> {
    match agent_runs::create(&ctx.db, org.org.id, org.auth.user.id, &input).await {
        Ok(run) => Ok((StatusCode::CREATED, Json(run)).into_response()),
        Err(e) => run_error(e),
    }
}

/// `GET /api/v1/agent-runs/{id}`
#[utoipa::path(
    get,
    path = "/api/v1/agent-runs/{id}",
    tag = "agent-runs",
    operation_id = "agent_runs_get",
    summary = "Get a run",
    description = "One run, its state and its LLM usage. Requires `agents:read`.",
    params(("id" = String, Path, format = "uuid", description = "Run id (UUID)")),
    responses(
        (status = 200, body = pnex_core::agent_runs::AgentRun),
        (status = 403, description = "`access-denied`", body = crate::openapi::ErrorDetail),
        (status = 404, description = "`agent-run-not-found`", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn one(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(id): Path<Uuid>,
) -> Result<Response> {
    match agent_runs::get(&ctx.db, org.org.id, id).await {
        Ok(run) => format::json(run),
        Err(e) => run_error(e),
    }
}

/// `POST /api/v1/agent-runs/{id}/stop`
#[utoipa::path(
    post,
    path = "/api/v1/agent-runs/{id}/stop",
    tag = "agent-runs",
    operation_id = "agent_runs_stop",
    summary = "Stop a run",
    description = "Ends the run now: its token stops working at once and its runner destroys the sandbox. Requires `agents:stop`.",
    params(("id" = String, Path, format = "uuid", description = "Run id (UUID)")),
    responses(
        (status = 200, body = pnex_core::agent_runs::AgentRun),
        (status = 403, description = "`access-denied`", body = crate::openapi::ErrorDetail),
        (status = 404, description = "`agent-run-not-found`", body = crate::openapi::ErrorDetail),
        (status = 409, description = "`agent-run-ended`", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn stop(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(id): Path<Uuid>,
) -> Result<Response> {
    match agent_runs::stop(&ctx.db, org.org.id, id).await {
        Ok(run) => format::json(run),
        Err(e) => run_error(e),
    }
}

/// `GET /api/v1/agent-runs/{id}/logs`
#[utoipa::path(
    get,
    path = "/api/v1/agent-runs/{id}/logs",
    tag = "agent-runs",
    operation_id = "agent_runs_logs",
    summary = "Read the journal of a run",
    description = "The last lines of each container of the sandbox, oldest first, in the order the containers ran (clone, services, setup, agent). Sent by the runner when the run ends, secrets masked. Requires `agents:read`. Paginated (`limit`/`offset`).",
    params(("id" = String, Path, format = "uuid", description = "Run id (UUID)"), PageQuery),
    responses(
        (status = 200, body = pnex_core::Paginated<pnex_core::runners::RunnerJobLogLine>),
        (status = 403, description = "`access-denied`", body = crate::openapi::ErrorDetail),
        (status = 404, description = "`agent-run-not-found`", body = crate::openapi::ErrorDetail),
        (status = 503, description = "`agent-run-logs-unavailable`", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn logs(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(id): Path<Uuid>,
    Query(q): Query<PageQuery>,
) -> Result<Response> {
    let (job, since_us) = match agent_runs::job_of(&ctx.db, org.org.id, id).await {
        Ok(found) => found,
        Err(e) => return run_error(e),
    };
    let page = PageParams::from(q.limit.as_deref(), q.offset.as_deref());
    let found =
        crate::services::job_logs::lines(&ctx, org.org.id, job, since_us, page.limit, page.offset)
            .await;
    let (count, rows) = match found {
        Ok(Some(found)) => found,
        not_read => {
            if let Err(e) = not_read {
                tracing::warn!(error = %e, "run journal not read");
            }
            return Err(super::coded_error(
                StatusCode::SERVICE_UNAVAILABLE,
                err_codes::AGENT_RUN_LOGS_UNAVAILABLE,
                "The journal of runs is not available.",
                None,
            ));
        }
    };
    format::json(pagination::envelope(
        &format!("/api/v1/agent-runs/{id}/logs"),
        &[],
        page,
        count,
        rows,
    ))
}

/// The run of the token in `x-api-key` (Anthropic engines) or `Authorization: Bearer`.
async fn run_of(ctx: &AppContext, headers: &HeaderMap) -> Result<RunPrincipal> {
    let token = headers
        .get("x-api-key")
        .and_then(|v| v.to_str().ok())
        .or_else(|| {
            headers
                .get(axum::http::header::AUTHORIZATION)
                .and_then(|v| v.to_str().ok())
                .and_then(|v| v.strip_prefix("Bearer "))
        });
    let found = match token {
        Some(t) => agent_runs::authenticate(&ctx.db, t.trim())
            .await
            .map_err(Error::DB)?,
        None => None,
    };
    found.ok_or_else(|| {
        super::coded_error(
            StatusCode::UNAUTHORIZED,
            err_codes::AGENT_RUN_UNAUTHORIZED,
            "Unknown run token, or the run is not running.",
            None,
        )
    })
}

fn gateway_error(e: GatewayError) -> Error {
    match e {
        GatewayError::NotConfigured => super::coded_error(
            StatusCode::SERVICE_UNAVAILABLE,
            err_codes::AGENT_GATEWAY_NOT_CONFIGURED,
            "The org has no LLM provider for agents.",
            None,
        ),
        GatewayError::ProtocolMismatch => super::coded_error(
            StatusCode::CONFLICT,
            err_codes::AGENT_GATEWAY_PROTOCOL_MISMATCH,
            "The provider of the org speaks the other protocol.",
            None,
        ),
        GatewayError::RunBudget => super::coded_error(
            StatusCode::TOO_MANY_REQUESTS,
            err_codes::AGENT_RUN_LLM_BUDGET,
            "The run has used all its LLM calls.",
            None,
        ),
        GatewayError::OrgQuota(cap) => super::coded_error(
            StatusCode::TOO_MANY_REQUESTS,
            err_codes::AGENT_GATEWAY_QUOTA,
            "The daily LLM calls of agents of the org are used up.",
            Some(serde_json::json!({ "cap": cap })),
        ),
        GatewayError::Invalid => super::coded_error(
            StatusCode::BAD_REQUEST,
            err_codes::AGENT_GATEWAY_INVALID,
            "The request body is not a JSON object.",
            None,
        ),
        GatewayError::Upstream(e) => {
            tracing::warn!(error = %e, "agent gateway upstream call failed");
            super::coded_error(
                StatusCode::BAD_GATEWAY,
                err_codes::AGENT_GATEWAY_UPSTREAM,
                "The LLM provider could not be reached.",
                None,
            )
        }
    }
}

async fn gateway(
    ctx: &AppContext,
    protocol: Protocol,
    headers: &HeaderMap,
    body: Bytes,
) -> Result<Response> {
    let run = run_of(ctx, headers).await?;
    agent_gateway::forward(ctx, run, protocol, headers, body)
        .await
        .map_err(gateway_error)
}

/// `POST /api/v1/agent-gateway/v1/messages`
#[utoipa::path(
    post,
    path = "/api/v1/agent-gateway/v1/messages",
    tag = "agent-runs",
    operation_id = "agent_gateway_messages",
    summary = "LLM call of a run, Anthropic protocol",
    description = "Called from a sandbox with its run token (`x-api-key` or bearer), while the run runs. Forwards a Messages request to the provider of the org for agents, with its model; the answer, streamed or not, comes back as the provider sent it. Counted against the run and the org.",
    request_body(content = serde_json::Value, description = "Messages request; its `model` is replaced by the org's"),
    responses(
        (status = 200, description = "Answer of the provider, JSON or event stream", body = serde_json::Value),
        (status = 400, description = "`agent-gateway-invalid`", body = crate::openapi::ErrorDetail),
        (status = 401, description = "`agent-run-unauthorized`", body = crate::openapi::ErrorDetail),
        (status = 409, description = "`agent-gateway-protocol-mismatch`", body = crate::openapi::ErrorDetail),
        (status = 429, description = "`agent-run-llm-budget` or `agent-gateway-quota` (args `cap`)", body = crate::openapi::ErrorDetail),
        (status = 502, description = "`agent-gateway-upstream`", body = crate::openapi::ErrorDetail),
        (status = 503, description = "`agent-gateway-not-configured`", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = []))
)]
async fn gateway_anthropic(
    State(ctx): State<AppContext>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Response> {
    gateway(&ctx, Protocol::Anthropic, &headers, body).await
}

/// `POST /api/v1/agent-gateway/v1/chat/completions`
#[utoipa::path(
    post,
    path = "/api/v1/agent-gateway/v1/chat/completions",
    tag = "agent-runs",
    operation_id = "agent_gateway_chat_completions",
    summary = "LLM call of a run, OpenAI protocol",
    description = "Called from a sandbox with its run token (bearer), while the run runs. Forwards a chat completion request to the provider of the org for agents, with its model; the answer, streamed or not, comes back as the provider sent it. Counted against the run and the org.",
    request_body(content = serde_json::Value, description = "Chat completion request; its `model` is replaced by the org's"),
    responses(
        (status = 200, description = "Answer of the provider, JSON or event stream", body = serde_json::Value),
        (status = 400, description = "`agent-gateway-invalid`", body = crate::openapi::ErrorDetail),
        (status = 401, description = "`agent-run-unauthorized`", body = crate::openapi::ErrorDetail),
        (status = 409, description = "`agent-gateway-protocol-mismatch`", body = crate::openapi::ErrorDetail),
        (status = 429, description = "`agent-run-llm-budget` or `agent-gateway-quota` (args `cap`)", body = crate::openapi::ErrorDetail),
        (status = 502, description = "`agent-gateway-upstream`", body = crate::openapi::ErrorDetail),
        (status = 503, description = "`agent-gateway-not-configured`", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = []))
)]
async fn gateway_openai(
    State(ctx): State<AppContext>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Response> {
    gateway(&ctx, Protocol::OpenAi, &headers, body).await
}
