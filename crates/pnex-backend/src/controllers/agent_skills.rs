//! Skills of hosted agents (agent-environments.md D711, autonomous-agents.md D396). The org
//! comes from the principal (R1), permissions are applied by `OrgContext` (R2):
//! `agents:read` to read, `agents:write` (Admin) to write a draft or activate a version,
//! `agents:delete` to remove. Every text lands as a draft; only a person activates one.

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::Json;
use loco_rs::controller::format;
use loco_rs::prelude::*;
use pnex_core::agent_skills::{
    AgentSkill, AgentSkillActivate, AgentSkillCreate, AgentSkillUpdate, AgentSkillVersion,
};
use pnex_core::err_codes;
use pnex_core::text_diff::TextDiff;
use uuid::Uuid;

use crate::auth::OrgContext;
use crate::controllers::pagination::{self, PageParams};
use crate::services::agent_skills::{self, SkillError};

pub fn routes() -> Routes {
    Routes::new()
        .prefix("/api/v1")
        .add("/agent-skills", get(list).post(create))
        .add("/agent-skills/{id}", get(one).patch(update).delete(remove))
        .add("/agent-skills/{id}/versions", get(versions))
        .add("/agent-skills/{id}/versions/{number}", get(version))
        .add("/agent-skills/{id}/diff", get(diff))
        .add("/agent-skills/{id}/activate", post(activate))
}

pub fn openapi() -> utoipa::openapi::OpenApi {
    #[derive(utoipa::OpenApi)]
    #[openapi(
        paths(list, create, one, update, remove, versions, version, diff, activate),
        components(schemas(
            AgentSkill,
            AgentSkillCreate,
            AgentSkillUpdate,
            AgentSkillVersion,
            AgentSkillActivate,
            pnex_core::agent_skills::SkillPin
        ))
    )]
    struct Doc;
    <Doc as utoipa::OpenApi>::openapi()
}

fn skill_error(e: SkillError) -> Result<Response> {
    Err(match e {
        SkillError::Invalid { field, token } => return Ok(super::field_error(&field, &token)),
        SkillError::NotFound => super::coded_error(
            StatusCode::NOT_FOUND,
            err_codes::AGENT_SKILL_NOT_FOUND,
            "Skill not found.",
            None,
        ),
        SkillError::VersionNotFound => super::coded_error(
            StatusCode::NOT_FOUND,
            err_codes::AGENT_SKILL_VERSION_NOT_FOUND,
            "The skill has no version with this number.",
            None,
        ),
        SkillError::NameTaken => super::coded_error(
            StatusCode::CONFLICT,
            err_codes::AGENT_SKILL_NAME_TAKEN,
            "A skill of the org already has this name.",
            None,
        ),
        SkillError::VersionConflict { current } => super::coded_error(
            StatusCode::CONFLICT,
            err_codes::AGENT_SKILL_VERSION_CONFLICT,
            "The skill changed since it was read; reload it.",
            Some(serde_json::json!({ "current": current.to_string() })),
        ),
        SkillError::Secret => super::coded_error(
            StatusCode::UNPROCESSABLE_ENTITY,
            err_codes::AGENT_SKILL_SECRET,
            "The text holds what looks like a credential; a skill never carries a secret.",
            None,
        ),
        SkillError::Db(e) => {
            tracing::error!(error = %e, "agent skill database error");
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

#[derive(serde::Deserialize, utoipa::IntoParams)]
#[into_params(parameter_in = Query)]
struct DiffQuery {
    /// Number of the older text.
    from: i32,
    /// Number of the newer text.
    to: i32,
}

/// `GET /api/v1/agent-skills`
#[utoipa::path(
    get,
    path = "/api/v1/agent-skills",
    tag = "agents",
    operation_id = "agent_skills_list",
    summary = "List the skills of the org",
    description = "Each skill with the number of its newest text and of its active one. Requires `agents:read`. Paginated (`limit`/`offset`).",
    params(PageQuery),
    responses(
        (status = 200, body = pnex_core::Paginated<AgentSkill>),
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
    let (count, rows) = agent_skills::list(&ctx.db, org.org.id, page.limit, page.offset)
        .await
        .map_err(Error::DB)?;
    format::json(pagination::envelope(
        "/api/v1/agent-skills",
        &[],
        page,
        count,
        rows,
    ))
}

/// `POST /api/v1/agent-skills`
#[utoipa::path(
    post,
    path = "/api/v1/agent-skills",
    tag = "agents",
    operation_id = "agent_skills_create",
    summary = "Create a skill",
    description = "Creates a skill whose text is its first draft; no agent can use it before a person activates a version. A text shaped like a credential is refused. Requires `agents:write` (Admin).",
    request_body = AgentSkillCreate,
    responses(
        (status = 201, body = AgentSkill),
        (status = 400, description = "`field-invalid` (`errors.fields`)", body = crate::openapi::ErrorDetail),
        (status = 403, description = "`access-denied`", body = crate::openapi::ErrorDetail),
        (status = 409, description = "`agent-skill-name-taken`", body = crate::openapi::ErrorDetail),
        (status = 422, description = "`agent-skill-secret`", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn create(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Json(body): Json<AgentSkillCreate>,
) -> Result<Response> {
    match agent_skills::create(&ctx.db, org.org.id, org.auth.user.id, &body).await {
        Ok(s) => Ok((StatusCode::CREATED, Json(s)).into_response()),
        Err(e) => skill_error(e),
    }
}

/// `GET /api/v1/agent-skills/{id}`
#[utoipa::path(
    get,
    path = "/api/v1/agent-skills/{id}",
    tag = "agents",
    operation_id = "agent_skills_get",
    summary = "Get a skill",
    description = "Requires `agents:read`.",
    params(("id" = String, Path, format = "uuid", description = "Skill id (UUID)")),
    responses(
        (status = 200, body = AgentSkill),
        (status = 403, description = "`access-denied`", body = crate::openapi::ErrorDetail),
        (status = 404, description = "`agent-skill-not-found`", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn one(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(id): Path<Uuid>,
) -> Result<Response> {
    match agent_skills::get(&ctx.db, org.org.id, id).await {
        Ok(s) => format::json(s),
        Err(e) => skill_error(e),
    }
}

/// `PATCH /api/v1/agent-skills/{id}`
#[utoipa::path(
    patch,
    path = "/api/v1/agent-skills/{id}",
    tag = "agents",
    operation_id = "agent_skills_update",
    summary = "Change a skill, a new text being a new draft",
    description = "On the version that was read (`expected_version`). A new `body` becomes the next draft; the active version stays until a person activates another. Requires `agents:write` (Admin).",
    params(("id" = String, Path, format = "uuid", description = "Skill id (UUID)")),
    request_body = AgentSkillUpdate,
    responses(
        (status = 200, body = AgentSkill),
        (status = 400, description = "`field-invalid` (`errors.fields`)", body = crate::openapi::ErrorDetail),
        (status = 403, description = "`access-denied`", body = crate::openapi::ErrorDetail),
        (status = 404, description = "`agent-skill-not-found`", body = crate::openapi::ErrorDetail),
        (status = 409, description = "`agent-skill-version-conflict` (args `current`)", body = crate::openapi::ErrorDetail),
        (status = 422, description = "`agent-skill-secret`", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn update(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(id): Path<Uuid>,
    Json(body): Json<AgentSkillUpdate>,
) -> Result<Response> {
    match agent_skills::update(&ctx.db, org.org.id, org.auth.user.id, id, &body).await {
        Ok(s) => format::json(s),
        Err(e) => skill_error(e),
    }
}

/// `DELETE /api/v1/agent-skills/{id}`
#[utoipa::path(
    delete,
    path = "/api/v1/agent-skills/{id}",
    tag = "agents",
    operation_id = "agent_skills_delete",
    summary = "Delete a skill",
    description = "Removes the skill and all its versions. An agent that still pins it can no longer start a run until its record drops it. Requires `agents:delete` (Admin).",
    params(("id" = String, Path, format = "uuid", description = "Skill id (UUID)")),
    responses(
        (status = 204, description = "Deleted"),
        (status = 403, description = "`access-denied`", body = crate::openapi::ErrorDetail),
        (status = 404, description = "`agent-skill-not-found`", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn remove(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(id): Path<Uuid>,
) -> Result<Response> {
    match agent_skills::delete(&ctx.db, org.org.id, id).await {
        Ok(()) => Ok(StatusCode::NO_CONTENT.into_response()),
        Err(e) => skill_error(e),
    }
}

/// `GET /api/v1/agent-skills/{id}/versions`
#[utoipa::path(
    get,
    path = "/api/v1/agent-skills/{id}/versions",
    tag = "agents",
    operation_id = "agent_skills_versions_list",
    summary = "List the texts of a skill",
    description = "Newest first, each with its text and whether it was ever activated. Requires `agents:read`. Paginated (`limit`/`offset`).",
    params(("id" = String, Path, format = "uuid", description = "Skill id (UUID)"), PageQuery),
    responses(
        (status = 200, body = pnex_core::Paginated<AgentSkillVersion>),
        (status = 403, description = "`access-denied`", body = crate::openapi::ErrorDetail),
        (status = 404, description = "`agent-skill-not-found`", body = crate::openapi::ErrorDetail)
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
    match agent_skills::versions(&ctx.db, org.org.id, id, page.limit, page.offset).await {
        Ok((count, rows)) => format::json(pagination::envelope(
            &format!("/api/v1/agent-skills/{id}/versions"),
            &[],
            page,
            count,
            rows,
        )),
        Err(e) => skill_error(e),
    }
}

/// `GET /api/v1/agent-skills/{id}/versions/{number}`
#[utoipa::path(
    get,
    path = "/api/v1/agent-skills/{id}/versions/{number}",
    tag = "agents",
    operation_id = "agent_skills_versions_get",
    summary = "Get one text of a skill",
    description = "Requires `agents:read`.",
    params(
        ("id" = String, Path, format = "uuid", description = "Skill id (UUID)"),
        ("number" = i32, Path, description = "Version number")
    ),
    responses(
        (status = 200, body = AgentSkillVersion),
        (status = 403, description = "`access-denied`", body = crate::openapi::ErrorDetail),
        (status = 404, description = "`agent-skill-not-found`, `agent-skill-version-not-found`", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn version(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path((id, number)): Path<(Uuid, i32)>,
) -> Result<Response> {
    match agent_skills::version(&ctx.db, org.org.id, id, number).await {
        Ok(v) => format::json(v),
        Err(e) => skill_error(e),
    }
}

/// `GET /api/v1/agent-skills/{id}/diff`
#[utoipa::path(
    get,
    path = "/api/v1/agent-skills/{id}/diff",
    tag = "agents",
    operation_id = "agent_skills_diff",
    summary = "Compare two texts of a skill",
    description = "Paragraph by paragraph, from `from` to `to`: what a person reads before activating a version. Requires `agents:read`.",
    params(("id" = String, Path, format = "uuid", description = "Skill id (UUID)"), DiffQuery),
    responses(
        (status = 200, body = TextDiff),
        (status = 403, description = "`access-denied`", body = crate::openapi::ErrorDetail),
        (status = 404, description = "`agent-skill-not-found`, `agent-skill-version-not-found`", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn diff(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(id): Path<Uuid>,
    Query(q): Query<DiffQuery>,
) -> Result<Response> {
    match agent_skills::diff(&ctx.db, org.org.id, id, q.from, q.to).await {
        Ok(d) => format::json(d),
        Err(e) => skill_error(e),
    }
}

/// `POST /api/v1/agent-skills/{id}/activate`
#[utoipa::path(
    post,
    path = "/api/v1/agent-skills/{id}/activate",
    tag = "agents",
    operation_id = "agent_skills_activate",
    summary = "Activate a version of a skill",
    description = "A person's decision, after reading its diff: from then on agents may pin this version. Never automatic, whoever wrote the text. On the version of the skill that was read (`expected_version`); a text shaped like a credential is refused. Requires `agents:write` (Admin).",
    params(("id" = String, Path, format = "uuid", description = "Skill id (UUID)")),
    request_body = AgentSkillActivate,
    responses(
        (status = 200, body = AgentSkill),
        (status = 403, description = "`access-denied`", body = crate::openapi::ErrorDetail),
        (status = 404, description = "`agent-skill-not-found`, `agent-skill-version-not-found`", body = crate::openapi::ErrorDetail),
        (status = 409, description = "`agent-skill-version-conflict` (args `current`)", body = crate::openapi::ErrorDetail),
        (status = 422, description = "`agent-skill-secret`", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn activate(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(id): Path<Uuid>,
    Json(body): Json<AgentSkillActivate>,
) -> Result<Response> {
    match agent_skills::activate(&ctx.db, org.org.id, org.auth.user.id, id, &body).await {
        Ok(s) => format::json(s),
        Err(e) => skill_error(e),
    }
}
