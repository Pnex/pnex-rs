//! Access management API (iam.md §8). What the caller may do in the
//! selected org, the catalogue of permissions, and (lot I3) the custom
//! roles, teams and assignments of the org, managed by owners and admins.
//! Lot I6: service accounts of the org and their access keys.
//! The permission of each operation is enforced by the `OrgContext`
//! extractor. Never exposed to the MCP nor to the assistant (iam.md §14).

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use chrono::{DateTime, FixedOffset};
use loco_rs::prelude::*;
use pnex_api_contract::iam::{Level, PERMISSIONS};
use pnex_core::err_codes;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::auth::OrgContext;
use crate::controllers::pagination;
use crate::models::_entities::sea_orm_active_enums::OrgMemberRole;
use crate::services::iam::service::{self, AccessKey, CreatedKey, ServiceAccount};
use crate::services::iam::store::{self, Assignment, CustomRole, IamError, Team};
use crate::services::iam::{self, Statement};

/// Lowest managed role holding a permission.
#[derive(Serialize, utoipa::ToSchema)]
#[serde(rename_all = "lowercase")]
enum RoleLevel {
    Viewer,
    Member,
    Admin,
    Owner,
}

impl From<Level> for RoleLevel {
    fn from(l: Level) -> Self {
        match l {
            Level::Viewer => Self::Viewer,
            Level::Member => Self::Member,
            Level::Admin => Self::Admin,
            Level::Owner => Self::Owner,
        }
    }
}

/// Effective rights of the caller in the selected org.
#[derive(Serialize, utoipa::ToSchema)]
struct MyPermissions {
    /// Managed role of the caller's membership in this org.
    role: RoleLevel,
    /// Every permission the caller holds (`service:verb`), membership role
    /// and assigned roles together, sorted.
    permissions: Vec<String>,
}

/// One permission of the registry.
#[derive(Serialize, utoipa::ToSchema)]
struct PermissionInfo {
    /// `service:verb`, e.g. `flows:deploy`.
    key: String,
    /// Lowest managed role that holds it.
    level: RoleLevel,
}

/// A role of the org: one of the four managed roles, or a custom role.
#[derive(Serialize, utoipa::ToSchema)]
struct RoleView {
    /// Custom role id; `null` for a managed role.
    #[schema(value_type = Option<String>, format = "uuid")]
    id: Option<Uuid>,
    key: String,
    name: String,
    description: String,
    /// Managed by the platform (not editable).
    managed: bool,
    statements: Vec<Statement>,
    /// Custom role version (`null` for a managed role).
    version: Option<i32>,
    #[schema(value_type = Option<String>, format = DateTime)]
    updated_at: Option<DateTime<FixedOffset>>,
}

#[derive(Deserialize, utoipa::IntoParams)]
#[into_params(parameter_in = Query)]
struct ListQuery {
    /// Case-insensitive substring of the key or name.
    #[serde(default)]
    search: Option<String>,
    #[param(value_type = Option<i64>)]
    limit: Option<String>,
    #[param(value_type = Option<i64>)]
    offset: Option<String>,
}

#[derive(Deserialize, utoipa::IntoParams)]
#[into_params(parameter_in = Query)]
struct AssignmentQuery {
    /// Only the assignments of this member (direct ones).
    #[serde(default)]
    user_id: Option<i64>,
    /// Only the assignments of this team.
    #[serde(default)]
    #[param(value_type = Option<String>, format = "uuid")]
    team_id: Option<Uuid>,
    /// Only the assignments of this custom role.
    #[serde(default)]
    #[param(value_type = Option<String>, format = "uuid")]
    role_id: Option<Uuid>,
    #[param(value_type = Option<i64>)]
    limit: Option<String>,
    #[param(value_type = Option<i64>)]
    offset: Option<String>,
}

/// New custom role.
#[derive(Deserialize, utoipa::ToSchema)]
struct RoleCreate {
    /// Slug, unique in the org (`a-z`, `0-9`, `-`), not a managed role key.
    key: String,
    name: String,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    statements: Vec<Statement>,
}

/// Partial update of a custom role.
#[derive(Deserialize, utoipa::ToSchema)]
struct RoleUpdate {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    description: Option<String>,
    /// Replaces every statement.
    #[serde(default)]
    statements: Option<Vec<Statement>>,
    /// Version read before editing (optimistic concurrency).
    expected_version: i32,
}

/// New team.
#[derive(Deserialize, utoipa::ToSchema)]
struct TeamCreate {
    name: String,
    #[serde(default)]
    description: Option<String>,
}

/// Partial update of a team.
#[derive(Deserialize, utoipa::ToSchema)]
struct TeamUpdate {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    description: Option<String>,
}

/// Member added to a team.
#[derive(Deserialize, utoipa::ToSchema)]
struct TeamMemberAdd {
    /// User id of a member of the org.
    user_id: i64,
}

/// Managed role an assignment may give (owner stays a membership role).
#[derive(Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "lowercase")]
enum AssignableRole {
    Viewer,
    Member,
    Admin,
}

/// New assignment: one holder (`user_id`, `team_id` or
/// `service_account_id`) and one role (`managed_role` or `role_id`).
#[derive(Deserialize, utoipa::ToSchema)]
struct AssignmentCreate {
    #[serde(default)]
    user_id: Option<i64>,
    #[serde(default)]
    #[schema(value_type = Option<String>, format = "uuid")]
    service_account_id: Option<Uuid>,
    #[serde(default)]
    #[schema(value_type = Option<String>, format = "uuid")]
    team_id: Option<Uuid>,
    #[serde(default)]
    managed_role: Option<AssignableRole>,
    #[serde(default)]
    #[schema(value_type = Option<String>, format = "uuid")]
    role_id: Option<Uuid>,
}

fn iam_error(e: IamError) -> Result<Response> {
    let not_found = |code: &str, what: &str| {
        Err(crate::controllers::coded_error(
            StatusCode::NOT_FOUND,
            code,
            format!("{what} not found"),
            None,
        ))
    };
    let conflict = |code: &str, what: &str| {
        Err(crate::controllers::coded_error(
            StatusCode::CONFLICT,
            code,
            what.to_string(),
            None,
        ))
    };
    match e {
        IamError::Invalid { field, token } => Ok(super::field_error(&field, &token)),
        IamError::RoleNotFound => not_found(err_codes::IAM_ROLE_NOT_FOUND, "role"),
        IamError::TeamNotFound => not_found(err_codes::IAM_TEAM_NOT_FOUND, "team"),
        IamError::AssignmentNotFound => {
            not_found(err_codes::IAM_ASSIGNMENT_NOT_FOUND, "assignment")
        }
        IamError::RoleKeyTaken => conflict(
            err_codes::IAM_ROLE_KEY_TAKEN,
            "a role of the org already has this key",
        ),
        IamError::TeamNameTaken => conflict(
            err_codes::IAM_TEAM_NAME_TAKEN,
            "a team of the org already has this name",
        ),
        IamError::AssignmentDuplicate => conflict(
            err_codes::IAM_ASSIGNMENT_DUPLICATE,
            "this holder already has this role",
        ),
        IamError::VersionConflict { current } => Err(crate::controllers::coded_error(
            StatusCode::CONFLICT,
            err_codes::IAM_VERSION_CONFLICT,
            "the role changed since it was read; reload it",
            Some(serde_json::json!({ "current": current.to_string() })),
        )),
        IamError::Escalation { permission } => Err(crate::controllers::coded_error(
            StatusCode::FORBIDDEN,
            err_codes::IAM_ESCALATION,
            format!("you cannot give {permission}: you do not hold it yourself"),
            Some(serde_json::json!({ "permission": permission })),
        )),
        IamError::ServiceAccountNotFound => {
            not_found(err_codes::IAM_SERVICE_ACCOUNT_NOT_FOUND, "service account")
        }
        IamError::ServiceAccountNameTaken => conflict(
            err_codes::IAM_SERVICE_ACCOUNT_NAME_TAKEN,
            "a service account of the org already has this name",
        ),
        IamError::KeyNotFound => {
            not_found(err_codes::IAM_ACCESS_KEY_NOT_FOUND, "active access key")
        }
        IamError::KeyLimit => conflict(
            err_codes::IAM_ACCESS_KEY_LIMIT,
            "two keys of this service account are active already: revoke one first",
        ),
        IamError::Db(e) => {
            tracing::error!(error = %e, "iam database error");
            Err(Error::InternalServerError)
        }
    }
}

/// In-memory page of a short list (roles and teams of one org).
fn page_of<T: Serialize>(
    path: &str,
    filters: Vec<(String, String)>,
    limit: Option<&str>,
    offset: Option<&str>,
    rows: Vec<T>,
) -> Result<Response> {
    let page = pagination::PageParams::from(limit, offset);
    let count = rows.len() as i64;
    let (skip, take) = page.slice(rows.len());
    format::json(pagination::envelope(
        path,
        &filters,
        page,
        count,
        rows.into_iter().skip(skip).take(take).collect(),
    ))
}

fn search_filter(search: &Option<String>) -> Vec<(String, String)> {
    search
        .iter()
        .map(|s| ("search".to_string(), s.clone()))
        .collect()
}

// ── Caller ──────────────────────────────────────────────────────────

#[utoipa::path(
    get,
    path = "/api/v1/iam/me/permissions",
    tag = "iam",
    operation_id = "iam_me_permissions",
    summary = "My permissions in the org",
    description = "Managed role of the caller in the selected org and every permission (`service:verb`) it holds with its assigned roles, so the UI hides what the server would refuse. The server still decides on every call. Any org member.",
    responses((status = 200, body = MyPermissions)),
    security(("bearer" = [], "org" = []))
)]
async fn me(org: OrgContext) -> Result<Response> {
    let permissions = org
        .grants
        .permissions()
        .into_iter()
        .map(str::to_string)
        .collect();
    format::json(MyPermissions {
        role: iam::level_of(&org.role).into(),
        permissions,
    })
}

#[utoipa::path(
    get,
    path = "/api/v1/iam/permissions",
    tag = "iam",
    operation_id = "iam_permissions_list",
    summary = "List the permissions",
    description = "Paginated catalogue of the permissions of the action registry (`service:verb`, sorted), each with the lowest managed role that holds it. Owner or admin.",
    params(ListQuery),
    responses((status = 200, body = pnex_core::Paginated<PermissionInfo>)),
    security(("bearer" = [], "org" = []))
)]
async fn permissions(_org: OrgContext, Query(q): Query<ListQuery>) -> Result<Response> {
    let rows: Vec<PermissionInfo> = PERMISSIONS
        .iter()
        .filter(|(p, _)| pagination::rust_search_match(&q.search, &[p]))
        .map(|(p, l)| PermissionInfo {
            key: p.to_string(),
            level: (*l).into(),
        })
        .collect();
    page_of(
        "/api/v1/iam/permissions",
        search_filter(&q.search),
        q.limit.as_deref(),
        q.offset.as_deref(),
        rows,
    )
}

// ── Roles ───────────────────────────────────────────────────────────

#[utoipa::path(
    get,
    path = "/api/v1/iam/roles",
    tag = "iam",
    operation_id = "iam_roles_list",
    summary = "List the roles",
    description = "The four managed roles (viewer, member, admin, owner; not editable) then the custom roles of the org, sorted by key, paginated. Owner or admin.",
    params(ListQuery),
    responses((status = 200, body = pnex_core::Paginated<RoleView>)),
    security(("bearer" = [], "org" = []))
)]
async fn roles_list(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Query(q): Query<ListQuery>,
) -> Result<Response> {
    let custom = match store::list_roles(&ctx.db, org.org.id).await {
        Ok(r) => r,
        Err(e) => return iam_error(e),
    };
    let managed = iam::managed_roles().iter().map(|r| RoleView {
        id: None,
        name: managed_name(&r.key).to_string(),
        key: r.key.clone(),
        description: String::new(),
        managed: true,
        statements: r.statements.clone(),
        version: None,
        updated_at: None,
    });
    let rows: Vec<RoleView> = managed
        .chain(custom.into_iter().map(|r| RoleView {
            id: Some(r.id),
            key: r.key,
            name: r.name,
            description: r.description,
            managed: false,
            statements: r.statements,
            version: Some(r.version),
            updated_at: Some(r.updated_at),
        }))
        .filter(|r| pagination::rust_search_match(&q.search, &[&r.key, &r.name]))
        .collect();
    page_of(
        "/api/v1/iam/roles",
        search_filter(&q.search),
        q.limit.as_deref(),
        q.offset.as_deref(),
        rows,
    )
}

fn managed_name(key: &str) -> &'static str {
    match key {
        "owner" => "Owner",
        "admin" => "Admin",
        "member" => "Member",
        _ => "Viewer",
    }
}

#[utoipa::path(
    get,
    path = "/api/v1/iam/roles/{id}",
    tag = "iam",
    operation_id = "iam_roles_get",
    summary = "Get a custom role",
    description = "A custom role of the org with its statements and version. Owner or admin.",
    params(("id" = String, Path, format = "uuid", description = "Custom role id")),
    responses(
        (status = 200, body = CustomRole),
        (status = 404, description = "`iam-role-not-found`", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn roles_get(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(id): Path<Uuid>,
) -> Result<Response> {
    match store::get_role(&ctx.db, org.org.id, id).await {
        Ok(r) => format::json(r),
        Err(e) => iam_error(e),
    }
}

const ROLE_400: &str = "`field-invalid`: `key` (`required`, `max_length:64`, `invalid`: not a slug or a managed role key), `name` (`required`, `max_length:120`), `description` (`max_length:2000`), `statements` (`max_items:50`), `statements.<i>.permissions` (`required`, `max_items:200`, `unknown_permission:<pattern>`, `iam_permission:<pattern>`: access permissions are never in a custom role)";

#[utoipa::path(
    post,
    path = "/api/v1/iam/roles",
    tag = "iam",
    operation_id = "iam_roles_create",
    summary = "Create a custom role",
    description = "Creates a custom role of the org: statements `{effect: allow|deny, permissions: [patterns]}`, a pattern being a permission (`flows:deploy`), a service (`flows:*`) or `*`. A custom role never grants nor takes away access management (`iam:*`). Anti-escalation: the caller must hold every permission the role allows. Owner or admin.",
    request_body = RoleCreate,
    responses(
        (status = 201, body = CustomRole),
        (status = 400, description = ROLE_400, body = crate::openapi::ErrorDetail),
        (status = 403, description = "`iam-escalation` (`args.permission`): the caller does not hold a permission the role allows", body = crate::openapi::ErrorDetail),
        (status = 409, description = "`iam-role-key-taken`", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn roles_create(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Json(input): Json<RoleCreate>,
) -> Result<Response> {
    let input = store::RoleInput {
        key: &input.key,
        name: &input.name,
        description: input.description.as_deref(),
        statements: &input.statements,
    };
    match store::create_role(&ctx.db, org.org.id, &org.grants, input).await {
        Ok(r) => Ok((StatusCode::CREATED, format::json(r)).into_response()),
        Err(e) => iam_error(e),
    }
}

#[utoipa::path(
    patch,
    path = "/api/v1/iam/roles/{id}",
    tag = "iam",
    operation_id = "iam_roles_update",
    summary = "Update a custom role",
    description = "Partial update of a custom role (name, description, statements replaced as a whole) on top of `expected_version`; every member holding it gets the change on their next call. Same pattern rules and anti-escalation as creation. Owner or admin.",
    params(("id" = String, Path, format = "uuid", description = "Custom role id")),
    request_body = RoleUpdate,
    responses(
        (status = 200, body = CustomRole),
        (status = 400, description = ROLE_400, body = crate::openapi::ErrorDetail),
        (status = 403, description = "`iam-escalation` (`args.permission`)", body = crate::openapi::ErrorDetail),
        (status = 404, description = "`iam-role-not-found`", body = crate::openapi::ErrorDetail),
        (status = 409, description = "`iam-version-conflict` (`args.current` = current version)", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn roles_update(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(id): Path<Uuid>,
    Json(input): Json<RoleUpdate>,
) -> Result<Response> {
    let patch = store::RolePatch {
        name: input.name.as_deref(),
        description: input.description.as_deref(),
        statements: input.statements.as_deref(),
        expected_version: input.expected_version,
    };
    match store::update_role(&ctx.db, org.org.id, id, &org.grants, patch).await {
        Ok(r) => format::json(r),
        Err(e) => iam_error(e),
    }
}

#[utoipa::path(
    delete,
    path = "/api/v1/iam/roles/{id}",
    tag = "iam",
    operation_id = "iam_roles_delete",
    summary = "Delete a custom role",
    description = "Deletes a custom role and every assignment of it. Owner or admin.",
    params(("id" = String, Path, format = "uuid", description = "Custom role id")),
    responses(
        (status = 204, description = "Deleted"),
        (status = 404, description = "`iam-role-not-found`", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn roles_delete(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(id): Path<Uuid>,
) -> Result<Response> {
    match store::delete_role(&ctx.db, org.org.id, id).await {
        Ok(()) => Ok(StatusCode::NO_CONTENT.into_response()),
        Err(e) => iam_error(e),
    }
}

// ── Teams ───────────────────────────────────────────────────────────

const TEAM_400: &str =
    "`field-invalid`: `name` (`required`, `max_length:120`), `description` (`max_length:2000`)";

#[utoipa::path(
    get,
    path = "/api/v1/iam/teams",
    tag = "iam",
    operation_id = "iam_teams_list",
    summary = "List the teams",
    description = "Teams of the org with the user ids of their members, sorted by name, paginated. Owner or admin.",
    params(ListQuery),
    responses((status = 200, body = pnex_core::Paginated<Team>)),
    security(("bearer" = [], "org" = []))
)]
async fn teams_list(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Query(q): Query<ListQuery>,
) -> Result<Response> {
    match store::list_teams(&ctx.db, org.org.id).await {
        Ok(rows) => page_of(
            "/api/v1/iam/teams",
            search_filter(&q.search),
            q.limit.as_deref(),
            q.offset.as_deref(),
            rows.into_iter()
                .filter(|t| pagination::rust_search_match(&q.search, &[&t.name]))
                .collect(),
        ),
        Err(e) => iam_error(e),
    }
}

#[utoipa::path(
    get,
    path = "/api/v1/iam/teams/{id}",
    tag = "iam",
    operation_id = "iam_teams_get",
    summary = "Get a team",
    description = "A team of the org with the user ids of its members. Owner or admin.",
    params(("id" = String, Path, format = "uuid", description = "Team id")),
    responses(
        (status = 200, body = Team),
        (status = 404, description = "`iam-team-not-found`", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn teams_get(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(id): Path<Uuid>,
) -> Result<Response> {
    match store::get_team(&ctx.db, org.org.id, id).await {
        Ok(t) => format::json(t),
        Err(e) => iam_error(e),
    }
}

#[utoipa::path(
    post,
    path = "/api/v1/iam/teams",
    tag = "iam",
    operation_id = "iam_teams_create",
    summary = "Create a team",
    description = "Creates an empty team of the org; give it roles with assignments, then add members. Owner or admin.",
    request_body = TeamCreate,
    responses(
        (status = 201, body = Team),
        (status = 400, description = TEAM_400, body = crate::openapi::ErrorDetail),
        (status = 409, description = "`iam-team-name-taken`", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn teams_create(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Json(input): Json<TeamCreate>,
) -> Result<Response> {
    match store::create_team(
        &ctx.db,
        org.org.id,
        &input.name,
        input.description.as_deref(),
    )
    .await
    {
        Ok(t) => Ok((StatusCode::CREATED, format::json(t)).into_response()),
        Err(e) => iam_error(e),
    }
}

#[utoipa::path(
    patch,
    path = "/api/v1/iam/teams/{id}",
    tag = "iam",
    operation_id = "iam_teams_update",
    summary = "Update a team",
    description = "Renames a team or changes its description. Owner or admin.",
    params(("id" = String, Path, format = "uuid", description = "Team id")),
    request_body = TeamUpdate,
    responses(
        (status = 200, body = Team),
        (status = 400, description = TEAM_400, body = crate::openapi::ErrorDetail),
        (status = 404, description = "`iam-team-not-found`", body = crate::openapi::ErrorDetail),
        (status = 409, description = "`iam-team-name-taken`", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn teams_update(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(id): Path<Uuid>,
    Json(input): Json<TeamUpdate>,
) -> Result<Response> {
    match store::update_team(
        &ctx.db,
        org.org.id,
        id,
        input.name.as_deref(),
        input.description.as_deref(),
    )
    .await
    {
        Ok(t) => format::json(t),
        Err(e) => iam_error(e),
    }
}

#[utoipa::path(
    delete,
    path = "/api/v1/iam/teams/{id}",
    tag = "iam",
    operation_id = "iam_teams_delete",
    summary = "Delete a team",
    description = "Deletes a team, its memberships and its assignments: its members lose the roles they held through it. Owner or admin.",
    params(("id" = String, Path, format = "uuid", description = "Team id")),
    responses(
        (status = 204, description = "Deleted"),
        (status = 404, description = "`iam-team-not-found`", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn teams_delete(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(id): Path<Uuid>,
) -> Result<Response> {
    match store::delete_team(&ctx.db, org.org.id, id).await {
        Ok(()) => Ok(StatusCode::NO_CONTENT.into_response()),
        Err(e) => iam_error(e),
    }
}

#[utoipa::path(
    post,
    path = "/api/v1/iam/teams/{id}/members",
    tag = "iam",
    operation_id = "iam_teams_members_add",
    summary = "Add a member to a team",
    description = "Adds a member of the org to the team (no-op when already in it); they get the team's roles on their next call. Owner or admin.",
    params(("id" = String, Path, format = "uuid", description = "Team id")),
    request_body = TeamMemberAdd,
    responses(
        (status = 200, body = Team),
        (status = 400, description = "`field-invalid`: `user_id` (`not_a_member`)", body = crate::openapi::ErrorDetail),
        (status = 404, description = "`iam-team-not-found`", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn teams_members_add(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(id): Path<Uuid>,
    Json(input): Json<TeamMemberAdd>,
) -> Result<Response> {
    match store::add_team_member(&ctx.db, org.org.id, id, input.user_id).await {
        Ok(t) => format::json(t),
        Err(e) => iam_error(e),
    }
}

#[utoipa::path(
    delete,
    path = "/api/v1/iam/teams/{id}/members/{user_id}",
    tag = "iam",
    operation_id = "iam_teams_members_remove",
    summary = "Remove a member from a team",
    description = "Removes a member from the team (no-op when not in it); they lose the team's roles on their next call. Owner or admin.",
    params(
        ("id" = String, Path, format = "uuid", description = "Team id"),
        ("user_id" = i64, Path, description = "User id of the member")
    ),
    responses(
        (status = 200, body = Team),
        (status = 404, description = "`iam-team-not-found`", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn teams_members_remove(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path((id, user_id)): Path<(Uuid, i64)>,
) -> Result<Response> {
    match store::remove_team_member(&ctx.db, org.org.id, id, user_id).await {
        Ok(t) => format::json(t),
        Err(e) => iam_error(e),
    }
}

// ── Assignments ─────────────────────────────────────────────────────

#[utoipa::path(
    get,
    path = "/api/v1/iam/assignments",
    tag = "iam",
    operation_id = "iam_assignments_list",
    summary = "List the assignments",
    description = "Roles given to members and teams of the org, oldest first, paginated; filter by `user_id` (direct assignments), `team_id` or `role_id`. Owner or admin.",
    params(AssignmentQuery),
    responses((status = 200, body = pnex_core::Paginated<Assignment>)),
    security(("bearer" = [], "org" = []))
)]
async fn assignments_list(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Query(q): Query<AssignmentQuery>,
) -> Result<Response> {
    let filter = store::AssignmentFilter {
        user_id: q.user_id,
        team_id: q.team_id,
        role_id: q.role_id,
    };
    let mut filters = Vec::new();
    if let Some(u) = q.user_id {
        filters.push(("user_id".to_string(), u.to_string()));
    }
    if let Some(t) = q.team_id {
        filters.push(("team_id".to_string(), t.to_string()));
    }
    if let Some(r) = q.role_id {
        filters.push(("role_id".to_string(), r.to_string()));
    }
    match store::list_assignments(&ctx.db, org.org.id, &filter).await {
        Ok(rows) => page_of(
            "/api/v1/iam/assignments",
            filters,
            q.limit.as_deref(),
            q.offset.as_deref(),
            rows,
        ),
        Err(e) => iam_error(e),
    }
}

#[utoipa::path(
    post,
    path = "/api/v1/iam/assignments",
    tag = "iam",
    operation_id = "iam_assignments_create",
    summary = "Give a role",
    description = "Gives a role to a member, a team or a service account over the whole org: exactly one of `user_id` / `team_id` / `service_account_id`, and one of `managed_role` (viewer, member, admin) / `role_id` (custom role). It adds to the membership role; a deny statement takes away. Applies on the holder's next call. Anti-escalation: the caller must hold every permission the role allows. Owner or admin.",
    request_body = AssignmentCreate,
    responses(
        (status = 201, body = Assignment),
        (status = 400, description = "`field-invalid`: `user_id` (`required`: no holder, `invalid`: two holders, `not_a_member`), `managed_role` (`required`: no role, `invalid`: two roles, `not_for_service_account`: admin), `role_id` (`invalid`: unknown role)", body = crate::openapi::ErrorDetail),
        (status = 403, description = "`iam-escalation` (`args.permission`)", body = crate::openapi::ErrorDetail),
        (status = 404, description = "`iam-team-not-found`, `iam-service-account-not-found`", body = crate::openapi::ErrorDetail),
        (status = 409, description = "`iam-assignment-duplicate`: this holder already has this role", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn assignments_create(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Json(input): Json<AssignmentCreate>,
) -> Result<Response> {
    let holder = match (input.user_id, input.team_id, input.service_account_id) {
        (Some(u), None, None) => store::Holder::User(u),
        (None, Some(t), None) => store::Holder::Team(t),
        (None, None, Some(sa)) => match service::account_user(&ctx.db, org.org.id, sa).await {
            Ok(u) => store::Holder::Service(u),
            Err(e) => return iam_error(e),
        },
        (None, None, None) => return Ok(super::field_error("user_id", "required")),
        _ => return Ok(super::field_error("user_id", "invalid")),
    };
    let given = match (input.managed_role, input.role_id) {
        (Some(m), None) => store::Given::Managed(match m {
            AssignableRole::Viewer => OrgMemberRole::Viewer,
            AssignableRole::Member => OrgMemberRole::Member,
            AssignableRole::Admin => OrgMemberRole::Admin,
        }),
        (None, Some(r)) => store::Given::Custom(r),
        (None, None) => return Ok(super::field_error("managed_role", "required")),
        (Some(_), Some(_)) => return Ok(super::field_error("managed_role", "invalid")),
    };
    match store::create_assignment(
        &ctx.db,
        org.org.id,
        org.auth.user.id,
        &org.grants,
        holder,
        given,
    )
    .await
    {
        Ok(a) => Ok((StatusCode::CREATED, format::json(a)).into_response()),
        Err(e) => iam_error(e),
    }
}

#[utoipa::path(
    delete,
    path = "/api/v1/iam/assignments/{id}",
    tag = "iam",
    operation_id = "iam_assignments_delete",
    summary = "Take a role back",
    description = "Deletes an assignment: the holder loses the role on their next call. Owner or admin.",
    params(("id" = String, Path, format = "uuid", description = "Assignment id")),
    responses(
        (status = 204, description = "Deleted"),
        (status = 404, description = "`iam-assignment-not-found`", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn assignments_delete(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(id): Path<Uuid>,
) -> Result<Response> {
    match store::delete_assignment(&ctx.db, org.org.id, id).await {
        Ok(()) => Ok(StatusCode::NO_CONTENT.into_response()),
        Err(e) => iam_error(e),
    }
}

// ── Service accounts and access keys ────────────────────────────────

/// New service account.
#[derive(Deserialize, utoipa::ToSchema)]
struct ServiceAccountCreate {
    name: String,
    #[serde(default)]
    description: Option<String>,
}

/// Partial update of a service account.
#[derive(Deserialize, utoipa::ToSchema)]
struct ServiceAccountUpdate {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    description: Option<String>,
}

/// New access key.
#[derive(Deserialize, utoipa::ToSchema)]
struct AccessKeyCreate {
    /// Mandatory expiry, in the future and one year away at most.
    #[schema(value_type = String, format = DateTime)]
    expires_at: DateTime<FixedOffset>,
}

const ACCOUNT_400: &str =
    "`field-invalid`: `name` (`required`, `max_length:120`), `description` (`max_length:2000`)";

#[utoipa::path(
    get,
    path = "/api/v1/iam/service-accounts",
    tag = "iam",
    operation_id = "iam_service_accounts_list",
    summary = "List the service accounts",
    description = "Non-human principals of the org, sorted by name, with their count of active keys, paginated. Owner or admin.",
    params(ListQuery),
    responses((status = 200, body = pnex_core::Paginated<ServiceAccount>)),
    security(("bearer" = [], "org" = []))
)]
async fn service_accounts_list(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Query(q): Query<ListQuery>,
) -> Result<Response> {
    match service::list_accounts(&ctx.db, org.org.id).await {
        Ok(rows) => page_of(
            "/api/v1/iam/service-accounts",
            search_filter(&q.search),
            q.limit.as_deref(),
            q.offset.as_deref(),
            rows.into_iter()
                .filter(|a| pagination::rust_search_match(&q.search, &[&a.name]))
                .collect(),
        ),
        Err(e) => iam_error(e),
    }
}

#[utoipa::path(
    get,
    path = "/api/v1/iam/service-accounts/{id}",
    tag = "iam",
    operation_id = "iam_service_accounts_get",
    summary = "Get a service account",
    description = "A service account of the org. Owner or admin.",
    params(("id" = String, Path, format = "uuid", description = "Service account id")),
    responses(
        (status = 200, body = ServiceAccount),
        (status = 404, description = "`iam-service-account-not-found`", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn service_accounts_get(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(id): Path<Uuid>,
) -> Result<Response> {
    match service::get_account(&ctx.db, org.org.id, id).await {
        Ok(a) => format::json(a),
        Err(e) => iam_error(e),
    }
}

#[utoipa::path(
    post,
    path = "/api/v1/iam/service-accounts",
    tag = "iam",
    operation_id = "iam_service_accounts_create",
    summary = "Create a service account",
    description = "Creates a non-human principal of the org for an external application on the server side (Next.js route handler, Dioxus server function). It holds nothing until it is given roles with assignments; it calls the org's read operations with an access key, never the access settings, the MCP or the platform. Owner or admin.",
    request_body = ServiceAccountCreate,
    responses(
        (status = 201, body = ServiceAccount),
        (status = 400, description = ACCOUNT_400, body = crate::openapi::ErrorDetail),
        (status = 409, description = "`iam-service-account-name-taken`", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn service_accounts_create(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Json(input): Json<ServiceAccountCreate>,
) -> Result<Response> {
    match service::create_account(
        &ctx.db,
        org.org.id,
        org.auth.user.id,
        &input.name,
        input.description.as_deref(),
    )
    .await
    {
        Ok(a) => Ok((StatusCode::CREATED, format::json(a)).into_response()),
        Err(e) => iam_error(e),
    }
}

#[utoipa::path(
    patch,
    path = "/api/v1/iam/service-accounts/{id}",
    tag = "iam",
    operation_id = "iam_service_accounts_update",
    summary = "Update a service account",
    description = "Renames a service account or changes its description. Owner or admin.",
    params(("id" = String, Path, format = "uuid", description = "Service account id")),
    request_body = ServiceAccountUpdate,
    responses(
        (status = 200, body = ServiceAccount),
        (status = 400, description = ACCOUNT_400, body = crate::openapi::ErrorDetail),
        (status = 404, description = "`iam-service-account-not-found`", body = crate::openapi::ErrorDetail),
        (status = 409, description = "`iam-service-account-name-taken`", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn service_accounts_update(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(id): Path<Uuid>,
    Json(input): Json<ServiceAccountUpdate>,
) -> Result<Response> {
    match service::update_account(
        &ctx.db,
        org.org.id,
        id,
        input.name.as_deref(),
        input.description.as_deref(),
    )
    .await
    {
        Ok(a) => format::json(a),
        Err(e) => iam_error(e),
    }
}

#[utoipa::path(
    delete,
    path = "/api/v1/iam/service-accounts/{id}",
    tag = "iam",
    operation_id = "iam_service_accounts_delete",
    summary = "Delete a service account",
    description = "Deletes a service account, its keys (refused at once) and its assignments. Owner or admin.",
    params(("id" = String, Path, format = "uuid", description = "Service account id")),
    responses(
        (status = 204, description = "Deleted"),
        (status = 404, description = "`iam-service-account-not-found`", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn service_accounts_delete(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(id): Path<Uuid>,
) -> Result<Response> {
    match service::delete_account(&ctx.db, org.org.id, id).await {
        Ok(()) => Ok(StatusCode::NO_CONTENT.into_response()),
        Err(e) => iam_error(e),
    }
}

#[utoipa::path(
    get,
    path = "/api/v1/iam/service-accounts/{id}/keys",
    tag = "iam",
    operation_id = "iam_access_keys_list",
    summary = "List the access keys",
    description = "Keys of a service account, newest first, without their secret: display prefix, expiry, last use, revocation. Owner or admin.",
    params(("id" = String, Path, format = "uuid", description = "Service account id"), ListQuery),
    responses(
        (status = 200, body = pnex_core::Paginated<AccessKey>),
        (status = 404, description = "`iam-service-account-not-found`", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn access_keys_list(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(id): Path<Uuid>,
    Query(q): Query<ListQuery>,
) -> Result<Response> {
    match service::list_keys(&ctx.db, org.org.id, id).await {
        Ok(rows) => page_of(
            &format!("/api/v1/iam/service-accounts/{id}/keys"),
            Vec::new(),
            q.limit.as_deref(),
            q.offset.as_deref(),
            rows,
        ),
        Err(e) => iam_error(e),
    }
}

#[utoipa::path(
    post,
    path = "/api/v1/iam/service-accounts/{id}/keys",
    tag = "iam",
    operation_id = "iam_access_keys_create",
    summary = "Create an access key",
    description = "Creates a key for the service account and answers its secret (`pnex_sk_…`) once: it is stored as a hash and never shown again. Use it server side only (`Authorization: Bearer pnex_sk_…`, `X-Org-Id` optional), never in a browser. Expiry mandatory, one year at most; two keys active at most, for rotation. Owner or admin.",
    params(("id" = String, Path, format = "uuid", description = "Service account id")),
    request_body = AccessKeyCreate,
    responses(
        (status = 201, body = CreatedKey),
        (status = 400, description = "`field-invalid`: `expires_at` (`in_past`, `max_days:366`)", body = crate::openapi::ErrorDetail),
        (status = 404, description = "`iam-service-account-not-found`", body = crate::openapi::ErrorDetail),
        (status = 409, description = "`iam-access-key-limit`: two keys are active already", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn access_keys_create(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(id): Path<Uuid>,
    Json(input): Json<AccessKeyCreate>,
) -> Result<Response> {
    match service::create_key(&ctx.db, org.org.id, id, org.auth.user.id, input.expires_at).await {
        Ok(k) => Ok((StatusCode::CREATED, format::json(k)).into_response()),
        Err(e) => iam_error(e),
    }
}

#[utoipa::path(
    delete,
    path = "/api/v1/iam/service-accounts/{id}/keys/{key_id}",
    tag = "iam",
    operation_id = "iam_access_keys_revoke",
    summary = "Revoke an access key",
    description = "Revokes a key at once: the next call with it answers 401. Owner or admin.",
    params(
        ("id" = String, Path, format = "uuid", description = "Service account id"),
        ("key_id" = String, Path, format = "uuid", description = "Access key id")
    ),
    responses(
        (status = 204, description = "Revoked"),
        (status = 404, description = "`iam-service-account-not-found`, `iam-access-key-not-found`", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn access_keys_revoke(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path((id, key_id)): Path<(Uuid, Uuid)>,
) -> Result<Response> {
    match service::revoke_key(&ctx.db, org.org.id, id, key_id).await {
        Ok(()) => Ok(StatusCode::NO_CONTENT.into_response()),
        Err(e) => iam_error(e),
    }
}

pub fn routes() -> Routes {
    Routes::new()
        .prefix("/api/v1/iam")
        .add("/me/permissions", get(me))
        .add("/permissions", get(permissions))
        .add("/roles", get(roles_list).post(roles_create))
        .add(
            "/roles/{id}",
            get(roles_get).patch(roles_update).delete(roles_delete),
        )
        .add("/teams", get(teams_list).post(teams_create))
        .add(
            "/teams/{id}",
            get(teams_get).patch(teams_update).delete(teams_delete),
        )
        .add("/teams/{id}/members", post(teams_members_add))
        .add(
            "/teams/{id}/members/{user_id}",
            delete(teams_members_remove),
        )
        .add(
            "/assignments",
            get(assignments_list).post(assignments_create),
        )
        .add("/assignments/{id}", delete(assignments_delete))
        .add(
            "/service-accounts",
            get(service_accounts_list).post(service_accounts_create),
        )
        .add(
            "/service-accounts/{id}",
            get(service_accounts_get)
                .patch(service_accounts_update)
                .delete(service_accounts_delete),
        )
        .add(
            "/service-accounts/{id}/keys",
            get(access_keys_list).post(access_keys_create),
        )
        .add(
            "/service-accounts/{id}/keys/{key_id}",
            delete(access_keys_revoke),
        )
}

/// OpenAPI paths of this controller (merged by `crate::openapi::spec`).
pub fn openapi() -> utoipa::openapi::OpenApi {
    #[derive(utoipa::OpenApi)]
    #[openapi(
        paths(
            me,
            permissions,
            roles_list,
            roles_get,
            roles_create,
            roles_update,
            roles_delete,
            teams_list,
            teams_get,
            teams_create,
            teams_update,
            teams_delete,
            teams_members_add,
            teams_members_remove,
            assignments_list,
            assignments_create,
            assignments_delete,
            service_accounts_list,
            service_accounts_get,
            service_accounts_create,
            service_accounts_update,
            service_accounts_delete,
            access_keys_list,
            access_keys_create,
            access_keys_revoke
        ),
        components(schemas(
            RoleLevel,
            RoleView,
            Team,
            Assignment,
            CustomRole,
            Statement,
            ServiceAccount,
            AccessKey,
            CreatedKey
        ))
    )]
    struct Doc;
    <Doc as utoipa::OpenApi>::openapi()
}
