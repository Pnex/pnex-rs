//! Agents of an org (agent-environments.md D757, D761). The org comes from the principal
//! (R1) and permissions are applied by `OrgContext` (R2): `agents:read` to read,
//! `agents:write` (Admin) to create or change, `agents:delete` to remove, `agents:run` to
//! start a run. The role of an agent never exceeds its author's.

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::Json;
use loco_rs::controller::format;
use loco_rs::prelude::*;
use pnex_core::agents::{Agent, AgentCreate, AgentRunStart, AgentUpdate, AgentVersion};
use pnex_core::err_codes;
use uuid::Uuid;

use crate::auth::OrgContext;
use crate::controllers::pagination::{self, PageParams};
use crate::services::agent_artifacts::ArtifactSettings;
use crate::services::agents::{self, AgentError};

pub fn routes() -> Routes {
    Routes::new()
        .prefix("/api/v1")
        .add("/agents", get(list).post(create))
        .add("/agents/{id}", get(one).put(update).delete(remove))
        .add("/agents/{id}/versions", get(versions))
        .add("/agents/{id}/runs", post(start_run))
}

pub fn openapi() -> utoipa::openapi::OpenApi {
    #[derive(utoipa::OpenApi)]
    #[openapi(
        paths(list, create, one, update, remove, versions, start_run),
        components(schemas(
            Agent,
            AgentCreate,
            AgentUpdate,
            AgentVersion,
            AgentRunStart,
            pnex_core::agents::AgentSpec,
            pnex_core::agents::AgentRole
        ))
    )]
    struct Doc;
    <Doc as utoipa::OpenApi>::openapi()
}

fn agent_error(e: AgentError) -> Result<Response> {
    Err(match e {
        AgentError::Invalid { field, token } => return Ok(super::field_error(&field, &token)),
        AgentError::NotFound => super::coded_error(
            StatusCode::NOT_FOUND,
            err_codes::AGENT_NOT_FOUND,
            "Agent not found.",
            None,
        ),
        AgentError::NameTaken => super::coded_error(
            StatusCode::CONFLICT,
            err_codes::AGENT_NAME_TAKEN,
            "An agent of the org already has this name.",
            None,
        ),
        AgentError::VersionConflict { current } => super::coded_error(
            StatusCode::CONFLICT,
            err_codes::AGENT_VERSION_CONFLICT,
            "The agent changed since it was read; reload it.",
            Some(serde_json::json!({ "current": current.to_string() })),
        ),
        AgentError::Quota { max } => super::coded_error(
            StatusCode::CONFLICT,
            err_codes::AGENT_QUOTA,
            "The tier of the org allows no more agents.",
            Some(serde_json::json!({ "max": max.to_string() })),
        ),
        AgentError::Escalation { permission } => super::coded_error(
            StatusCode::FORBIDDEN,
            err_codes::IAM_ESCALATION,
            format!("you cannot give {permission}: you do not hold it yourself"),
            Some(serde_json::json!({ "permission": permission })),
        ),
        AgentError::ProviderNotFound => super::coded_error(
            StatusCode::NOT_FOUND,
            err_codes::AGENT_PROVIDER_NOT_FOUND,
            "The agent names no LLM provider of the org.",
            None,
        ),
        AgentError::Run(e) => return super::agent_runs::run_error(e),
        AgentError::Db(e) => {
            tracing::error!(error = %e, "agent database error");
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

/// `GET /api/v1/agents`
#[utoipa::path(
    get,
    path = "/api/v1/agents",
    tag = "agents",
    operation_id = "agents_list",
    summary = "List the agents of the org",
    description = "Each agent with its identity, role and the current version of its record. Requires `agents:read`. Paginated (`limit`/`offset`).",
    params(PageQuery),
    responses(
        (status = 200, body = pnex_core::Paginated<Agent>),
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
    let (count, rows) = agents::list(&ctx.db, org.org.id, page.limit, page.offset)
        .await
        .map_err(Error::DB)?;
    format::json(pagination::envelope(
        "/api/v1/agents",
        &[],
        page,
        count,
        rows,
    ))
}

/// `POST /api/v1/agents`
#[utoipa::path(
    post,
    path = "/api/v1/agents",
    tag = "agents",
    operation_id = "agents_create",
    summary = "Create an agent",
    description = "Creates a service account of the org for the agent, gives it its role (`viewer` by default, `member` at most, never above the caller's), and the first version of its record. Requires `agents:write` (Admin).",
    request_body = AgentCreate,
    responses(
        (status = 201, body = Agent),
        (status = 400, description = "`field-invalid` (`errors.fields`)", body = crate::openapi::ErrorDetail),
        (status = 403, description = "`access-denied`, `iam-escalation` (args `permission`)", body = crate::openapi::ErrorDetail),
        (status = 404, description = "`agent-provider-not-found`", body = crate::openapi::ErrorDetail),
        (status = 409, description = "`agent-name-taken`, `agent-quota` (args `max`)", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn create(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Json(body): Json<AgentCreate>,
) -> Result<Response> {
    match agents::create(&ctx.db, org.org.id, org.auth.user.id, &org.grants, &body).await {
        Ok(a) => Ok((StatusCode::CREATED, Json(a)).into_response()),
        Err(e) => agent_error(e),
    }
}

/// `GET /api/v1/agents/{id}`
#[utoipa::path(
    get,
    path = "/api/v1/agents/{id}",
    tag = "agents",
    operation_id = "agents_get",
    summary = "Get an agent",
    description = "Requires `agents:read`.",
    params(("id" = String, Path, format = "uuid", description = "Agent id (UUID)")),
    responses(
        (status = 200, body = Agent),
        (status = 403, description = "`access-denied`", body = crate::openapi::ErrorDetail),
        (status = 404, description = "`agent-not-found`", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn one(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(id): Path<Uuid>,
) -> Result<Response> {
    match agents::get(&ctx.db, org.org.id, id).await {
        Ok(a) => format::json(a),
        Err(e) => agent_error(e),
    }
}

/// `PUT /api/v1/agents/{id}`
#[utoipa::path(
    put,
    path = "/api/v1/agents/{id}",
    tag = "agents",
    operation_id = "agents_update",
    summary = "Save a new version of an agent",
    description = "Replaces the record on the version that was read (`expected_version`); the previous one stays in its versions. Requires `agents:write` (Admin).",
    params(("id" = String, Path, format = "uuid", description = "Agent id (UUID)")),
    request_body = AgentUpdate,
    responses(
        (status = 200, body = Agent),
        (status = 400, description = "`field-invalid` (`errors.fields`)", body = crate::openapi::ErrorDetail),
        (status = 403, description = "`access-denied`", body = crate::openapi::ErrorDetail),
        (status = 404, description = "`agent-not-found`, `agent-provider-not-found`", body = crate::openapi::ErrorDetail),
        (status = 409, description = "`agent-version-conflict` (args `current`)", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn update(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(id): Path<Uuid>,
    Json(body): Json<AgentUpdate>,
) -> Result<Response> {
    match agents::update(&ctx.db, org.org.id, org.auth.user.id, id, &body).await {
        Ok(a) => format::json(a),
        Err(e) => agent_error(e),
    }
}

/// `DELETE /api/v1/agents/{id}`
#[utoipa::path(
    delete,
    path = "/api/v1/agents/{id}",
    tag = "agents",
    operation_id = "agents_delete",
    summary = "Delete an agent",
    description = "Removes the agent and its service account, keys and roles with it; its runs keep their trace. Requires `agents:delete` (Admin).",
    params(("id" = String, Path, format = "uuid", description = "Agent id (UUID)")),
    responses(
        (status = 204, description = "Deleted"),
        (status = 403, description = "`access-denied`", body = crate::openapi::ErrorDetail),
        (status = 404, description = "`agent-not-found`", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn remove(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(id): Path<Uuid>,
) -> Result<Response> {
    match agents::delete(&ctx.db, org.org.id, id).await {
        Ok(()) => Ok(StatusCode::NO_CONTENT.into_response()),
        Err(e) => agent_error(e),
    }
}

/// `GET /api/v1/agents/{id}/versions`
#[utoipa::path(
    get,
    path = "/api/v1/agents/{id}/versions",
    tag = "agents",
    operation_id = "agents_versions",
    summary = "List the versions of an agent",
    description = "Newest first. Requires `agents:read`.",
    params(("id" = String, Path, format = "uuid", description = "Agent id (UUID)"), PageQuery),
    responses(
        (status = 200, body = pnex_core::Paginated<AgentVersion>),
        (status = 403, description = "`access-denied`", body = crate::openapi::ErrorDetail),
        (status = 404, description = "`agent-not-found`", body = crate::openapi::ErrorDetail)
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
    match agents::versions(&ctx.db, org.org.id, id).await {
        Ok(all) => {
            let count = all.len() as i64;
            // shortcut: versions read whole then sliced; page in SQL if a record ever has
            // thousands of versions.
            let rows: Vec<AgentVersion> = all
                .into_iter()
                .skip(page.offset.max(0) as usize)
                .take(page.limit.max(0) as usize)
                .collect();
            format::json(pagination::envelope(
                &format!("/api/v1/agents/{id}/versions"),
                &[],
                page,
                count,
                rows,
            ))
        }
        Err(e) => agent_error(e),
    }
}

/// `POST /api/v1/agents/{id}/runs`
#[utoipa::path(
    post,
    path = "/api/v1/agents/{id}/runs",
    tag = "agents",
    operation_id = "agents_runs_start",
    summary = "Start a run of an agent",
    description = "Runs the current version of the record as the agent, given the files for its declared inputs. A timeout or an LLM budget may only be lower than the record's (D380). Requires `agents:run`.",
    params(("id" = String, Path, format = "uuid", description = "Agent id (UUID)")),
    request_body = AgentRunStart,
    responses(
        (status = 201, body = pnex_core::agent_runs::AgentRun),
        (status = 400, description = "`field-invalid` (`above_agent`, `undeclared`, `missing`)", body = crate::openapi::ErrorDetail),
        (status = 403, description = "`access-denied`", body = crate::openapi::ErrorDetail),
        (status = 404, description = "`agent-not-found`, `environment-not-found`, `agent-artifact-not-found`", body = crate::openapi::ErrorDetail),
        (status = 409, description = "`agent-run-no-active-build`", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn start_run(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(id): Path<Uuid>,
    Json(body): Json<AgentRunStart>,
) -> Result<Response> {
    let settings = ArtifactSettings::from_config(&ctx.config);
    match agents::start_run(&ctx.db, settings, org.org.id, org.auth.user.id, id, &body).await {
        Ok(run) => Ok((StatusCode::CREATED, Json(run)).into_response()),
        Err(e) => agent_error(e),
    }
}
