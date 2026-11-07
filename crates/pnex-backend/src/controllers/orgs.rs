//! Organizations and their members management (multi-tenant, decision D2).
//!
//! Access rules (SEC-75): every `/orgs/{id}…` route of the org goes through
//! `OrgContext`, so the registry permission (`orgs:*`, `orgs.members:*`) is
//! evaluated and a refusal audited (D294); the path org must be the
//! `X-Org-Id` one. Owner-only acts (grant the owner role, change or remove
//! an owner) are the `orgs.owners:write` gate. At least one owner always
//! remains; deleting an org needs its owner to be its last member.
//!
//! Joining (SEC-72): an admin invites an email, the person accepts. The
//! invitation answer is the same whether an account uses that email or not,
//! and it waits for a sign-up with that verified email until it expires.

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use loco_rs::prelude::*;
use pnex_core::OrgRole;
use sea_orm::{
    ActiveModelTrait, ColumnTrait, EntityTrait, ExprTrait, PaginatorTrait, QueryFilter, Set,
};
use sea_orm::{ConnectionTrait, DatabaseBackend, QueryOrder, Statement, TransactionTrait};
use serde::Deserialize;

use super::pagination;
use crate::auth::{AuthUser, OrgContext};
use crate::models::_entities::{
    organization_members, organizations, sea_orm_active_enums::OrgMemberRole, subscription_tiers,
    users,
};

/// Invitations wait this long for an answer.
const INVITATION_TTL_DAYS: i32 = 14;

fn stmt(sql: &str, values: Vec<sea_orm::Value>) -> Statement {
    Statement::from_sql_and_values(DatabaseBackend::Postgres, sql, values)
}

fn db_error(_: sea_orm::DbErr) -> Error {
    Error::InternalServerError
}

/// The org of the path must be the one `OrgContext` checked (`X-Org-Id`).
fn same_org(org: &OrgContext, path_org: i64) -> Result<()> {
    if org.org.id == path_org {
        return Ok(());
    }
    Err(crate::controllers::coded_error(
        StatusCode::FORBIDDEN,
        pnex_core::err_codes::ORG_PATH_MISMATCH,
        "the organisation of the path is not the X-Org-Id one",
        None,
    ))
}

/// Membership + org for (user, org), if the user is a member.
async fn membership_of(
    db: &sea_orm::DatabaseConnection,
    user_id: i64,
    org_id: i64,
) -> Result<Option<(organization_members::Model, organizations::Model)>> {
    let row = organization_members::Entity::find()
        .filter(
            organization_members::Column::UserId
                .eq(user_id)
                .and(organization_members::Column::OrgId.eq(org_id)),
        )
        .find_also_related(organizations::Entity)
        .one(db)
        .await
        .map_err(|_| Error::InternalServerError)?;
    // find_also_related on a mandatory FK: the org always exists.
    Ok(row
        .filter(|(_, org)| org.is_some())
        .map(|(m, org)| (m, org.unwrap())))
}

// Error helpers carry a machine code (`pnex_core::err_codes` registry) so
// the frontend can resolve `err-<kebab>` at render time; the English text is
// the verbatim fallback for unregistered codes.
fn forbidden(code: &str, msg: &str) -> Error {
    Error::CustomError(
        StatusCode::FORBIDDEN,
        loco_rs::controller::ErrorDetail::new(code, msg.to_string()),
    )
}

fn conflict(code: &str, msg: &str) -> Error {
    Error::CustomError(
        StatusCode::CONFLICT,
        loco_rs::controller::ErrorDetail::new(code, msg.to_string()),
    )
}

/// Role as exposed by the API (lowercase enum `pnex_core::OrgRole`).
pub fn api_role(role: OrgMemberRole) -> OrgRole {
    match role {
        OrgMemberRole::Owner => OrgRole::Owner,
        OrgMemberRole::Admin => OrgRole::Admin,
        OrgMemberRole::Member => OrgRole::Member,
        OrgMemberRole::Viewer => OrgRole::Viewer,
    }
}

/// Storage form of an API role.
fn db_role(role: OrgRole) -> OrgMemberRole {
    match role {
        OrgRole::Owner => OrgMemberRole::Owner,
        OrgRole::Admin => OrgMemberRole::Admin,
        OrgRole::Member => OrgMemberRole::Member,
        OrgRole::Viewer => OrgMemberRole::Viewer,
    }
}

/// Answer of `POST /api/v1/orgs` and of an accepted invitation (shaped
/// like `OrgSummary`).
#[derive(serde::Serialize, utoipa::ToSchema)]
struct OrgCreated {
    id: i64,
    name: String,
    /// Always `owner`.
    role: OrgRole,
}

/// Answer of `PATCH /api/v1/orgs/{id}`.
#[derive(serde::Serialize, utoipa::ToSchema)]
struct OrgRenamed {
    id: i64,
    name: String,
}

/// Answer of `PATCH /api/v1/orgs/{id}/members/{user_id}`.
#[derive(serde::Serialize, utoipa::ToSchema)]
struct MemberRoleUpdated {
    user_id: i64,
    role: OrgRole,
}

// ─────────────────────────────── Orgs ───────────────────────────────

#[derive(Debug, Default, Deserialize, utoipa::IntoParams)]
#[into_params(parameter_in = Query)]
struct ListOrgsQuery {
    /// Case-insensitive search on the org name.
    search: Option<String>,
    #[param(value_type = Option<i64>)]
    limit: Option<String>,
    #[param(value_type = Option<i64>)]
    offset: Option<String>,
}

/// Tier of a new org — `PNEX_DEFAULT_ORG_TIER` (dev: "Admin" to
/// unbridle the test account), Free otherwise; Free fallback if the named tier
/// does not exist (never an org without a tier because of a config typo).
pub(crate) async fn default_org_tier<C>(db: &C) -> Option<i64>
where
    C: sea_orm::ConnectionTrait,
{
    let wanted = std::env::var("PNEX_DEFAULT_ORG_TIER").unwrap_or_else(|_| "Free".into());
    let tiers = subscription_tiers::Entity::find().all(db).await.ok()?;
    let wanted = wanted.trim();
    tiers
        .iter()
        .find(|t| t.name.eq_ignore_ascii_case(wanted))
        .or_else(|| tiers.iter().find(|t| t.name == "Free"))
        .map(|t| t.id)
}

/// `GET /api/v1/orgs` — orgs I am a member of (with role and tier),
/// paginated (D14) + search on the name.
#[utoipa::path(
    get,
    path = "/api/v1/orgs",
    tag = "orgs",
    operation_id = "orgs_list",
    summary = "List my organisations",
    description = "Organisations the caller is a member of, with their role and subscription tier name. Paginated (`limit`/`offset`), optional name search. Not scoped by X-Org-Id.",
    params(ListOrgsQuery),
    responses(
        (status = 200, body = pnex_core::Paginated<pnex_core::OrgSummary>)
    ),
    security(("bearer" = []))
)]
async fn list(
    State(ctx): State<AppContext>,
    auth: AuthUser,
    Query(q): Query<ListOrgsQuery>,
) -> Result<Response> {
    let page = pagination::PageParams::from(q.limit.as_deref(), q.offset.as_deref());
    let memberships = organization_members::Entity::find()
        .filter(organization_members::Column::UserId.eq(auth.user.id))
        // Oldest first: without an order, pages would move between two reads.
        .order_by_asc(organization_members::Column::OrgId)
        .find_also_related(organizations::Entity)
        .all(&ctx.db)
        .await
        .map_err(|_| Error::InternalServerError)?;

    let tiers: std::collections::HashMap<i64, String> = subscription_tiers::Entity::find()
        .all(&ctx.db)
        .await
        .map_err(|_| Error::InternalServerError)?
        .into_iter()
        .map(|t| (t.id, t.name))
        .collect();

    // The set (a user's orgs) is bounded by nature: Rust filter then
    // slicing — the count reflects the filtered total.
    let orgs: Vec<serde_json::Value> = memberships
        .into_iter()
        .filter_map(|(m, org)| org.map(|o| (m, o)))
        .filter(|(_, o)| pagination::rust_search_match(&q.search, &[o.name.as_str()]))
        .map(|(m, o)| {
            serde_json::json!({
                "id": o.id,
                "name": o.name,
                "role": api_role(m.role),
                "subscription_tier": o.subscription_tier_id
                    .and_then(|id| tiers.get(&id).cloned()),
                "created_at": o.created_at,
            })
        })
        .collect();
    let count = orgs.len() as i64;
    let (skip, take) = page.slice(orgs.len());
    let filters = q
        .search
        .map(|s| vec![("search".to_string(), s)])
        .unwrap_or_default();
    format::json(pagination::envelope(
        "/api/v1/orgs",
        &filters,
        page,
        count,
        orgs.into_iter().skip(skip).take(take).collect(),
    ))
}

#[derive(Deserialize)]
struct CreateOrgParams {
    name: String,
}

/// `POST /api/v1/orgs` — creation, the creator becomes owner (Free tier).
#[utoipa::path(
    post,
    path = "/api/v1/orgs",
    tag = "orgs",
    operation_id = "orgs_create",
    summary = "Create an organisation",
    description = "Creates an org with the default subscription tier; the caller becomes its owner. Names are globally unique.",
    request_body = pnex_core::CreateOrg,
    responses(
        (status = 201, body = OrgCreated),
        (status = 400, description = "`field-invalid`: empty name (`errors.fields.name = required`)", body = crate::openapi::ErrorDetail),
        (status = 409, description = "Name already used (`org-name-duplicate`)", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = []))
)]
async fn create(
    State(ctx): State<AppContext>,
    auth: AuthUser,
    Json(params): Json<CreateOrgParams>,
) -> Result<Response> {
    let name = params.name.trim().to_string();
    if name.is_empty() {
        return Ok(crate::controllers::field_error(
            "name",
            pnex_core::err_codes::FIELD_REQUIRED,
        ));
    }
    if organizations::Entity::find()
        .filter(organizations::Column::Name.eq(&name))
        .one(&ctx.db)
        .await
        .map_err(|_| Error::InternalServerError)?
        .is_some()
    {
        return Err(conflict(
            "org-name-duplicate",
            "An organization already uses this name.",
        ));
    }

    let free_tier = default_org_tier(&ctx.db).await;

    let org = organizations::ActiveModel {
        name: Set(name),
        subscription_tier_id: Set(free_tier),
        ..Default::default()
    }
    .insert(&ctx.db)
    .await
    .map_err(|_| Error::InternalServerError)?;

    organization_members::ActiveModel {
        org_id: Set(org.id),
        user_id: Set(auth.user.id),
        role: Set(OrgMemberRole::Owner),
        ..Default::default()
    }
    .insert(&ctx.db)
    .await
    .map_err(|_| Error::InternalServerError)?;

    Ok((
        StatusCode::CREATED,
        // Shaped like `OrgSummary`: the UI decodes it and activates the new org.
        format::json(OrgCreated {
            id: org.id,
            name: org.name,
            role: api_role(OrgMemberRole::Owner),
        }),
    )
        .into_response())
}

/// `GET /api/v1/orgs/:id` — detail (members included).
#[utoipa::path(
    get,
    path = "/api/v1/orgs/{id}",
    tag = "orgs",
    operation_id = "orgs_get",
    summary = "Get an organisation with its members",
    description = "Org detail, the caller role and the full member list. Any member (`orgs:read`); `X-Org-Id` must be the path org.",
    params(("id" = i64, Path, description = "Org id")),
    responses(
        (status = 200, body = pnex_core::OrgDetail),
        (status = 403, description = "`org-path-mismatch`: the path org is not the `X-Org-Id` one", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn detail(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(org_id): Path<i64>,
) -> Result<Response> {
    same_org(&org, org_id)?;
    let members = list_members_json(&ctx.db, org_id).await?;
    format::json(serde_json::json!({
        "id": org.org.id,
        "name": org.org.name,
        "subscription_tier_id": org.org.subscription_tier_id,
        "role": api_role(org.role),
        "members": members,
    }))
}

#[derive(Deserialize)]
struct UpdateOrgParams {
    name: String,
}

/// `PATCH /api/v1/orgs/:id` — rename (`orgs:write`).
#[utoipa::path(
    patch,
    path = "/api/v1/orgs/{id}",
    tag = "orgs",
    operation_id = "orgs_update",
    summary = "Rename an organisation",
    description = "Renames the org. Needs `orgs:write` (owner or admin); `X-Org-Id` must be the path org. Names are globally unique.",
    params(("id" = i64, Path, description = "Org id")),
    request_body = pnex_core::UpdateOrg,
    responses(
        (status = 200, body = OrgRenamed),
        (status = 400, description = "`field-invalid`: empty name (`errors.fields.name = required`)", body = crate::openapi::ErrorDetail),
        (status = 403, description = "`org-path-mismatch`: the path org is not the `X-Org-Id` one", body = crate::openapi::ErrorDetail),
        (status = 409, description = "Name already used (`org-name-duplicate`)", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn update(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(org_id): Path<i64>,
    Json(params): Json<UpdateOrgParams>,
) -> Result<Response> {
    same_org(&org, org_id)?;
    let name = params.name.trim().to_string();
    if name.is_empty() {
        return Ok(crate::controllers::field_error(
            "name",
            pnex_core::err_codes::FIELD_REQUIRED,
        ));
    }
    let taken = organizations::Entity::find()
        .filter(
            organizations::Column::Name
                .eq(&name)
                .and(organizations::Column::Id.ne(org_id)),
        )
        .one(&ctx.db)
        .await
        .map_err(|_| Error::InternalServerError)?
        .is_some();
    if taken {
        return Err(conflict(
            "org-name-duplicate",
            "An organization already uses this name.",
        ));
    }

    let mut active: organizations::ActiveModel = org.org.into();
    active.name = Set(name);
    let org = active
        .update(&ctx.db)
        .await
        .map_err(|_| Error::InternalServerError)?;
    format::json(OrgRenamed {
        id: org.id,
        name: org.name,
    })
}

/// `DELETE /api/v1/orgs/:id` — owner (`orgs:delete`) and last member only.
#[utoipa::path(
    delete,
    path = "/api/v1/orgs/{id}",
    tag = "orgs",
    operation_id = "orgs_delete",
    summary = "Delete an organisation",
    description = "Deletes the org and, by cascade, all its data. Needs `orgs:delete` (owner), and the caller must be the last member; `X-Org-Id` must be the path org. Telemetry streams are purged from OpenObserve in the background (best effort).",
    params(("id" = i64, Path, description = "Org id")),
    responses(
        (status = 204, description = "Deleted"),
        (status = 403, description = "`org-path-mismatch`: the path org is not the `X-Org-Id` one", body = crate::openapi::ErrorDetail),
        (status = 409, description = "Other members remain (`org-not-empty`)", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn delete(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(org_id): Path<i64>,
) -> Result<Response> {
    same_org(&org, org_id)?;
    let org = org.org;
    let member_count = organization_members::Entity::find()
        .filter(organization_members::Column::OrgId.eq(org_id))
        .count(&ctx.db)
        .await
        .map_err(|_| Error::InternalServerError)?;
    if member_count > 1 {
        return Err(conflict(
            "org-not-empty",
            "The organization must be empty — remove the other members first",
        ));
    }

    // Telemetry lives outside the DB: read the O2 identifier before the
    // cascade drops `openobserve_orgs`, then purge its streams in the
    // background (best-effort, D72 — an O2 outage never blocks the delete).
    let o2_creds = crate::services::openobserve::provisioned_credentials(&ctx.db, org.id)
        .await
        .ok()
        .flatten();

    organizations::Entity::delete_by_id(org.id)
        .exec(&ctx.db)
        .await
        .map_err(|_| Error::InternalServerError)?;

    if let (Some(creds), Some(settings)) = (
        o2_creds,
        crate::services::openobserve::OpenobserveSettings::from_config(&ctx.config),
    ) {
        tokio::spawn(async move {
            // Retried with backoff (O2 restart, rolling update): the org row
            // is already gone, so this task is the only trace of the purge.
            // A final failure is logged at error level with the O2 org id
            // for a manual purge (not durable across a pod restart).
            let client = crate::services::openobserve::Client::new(&settings);
            let delays = [5u64, 30, 120, 600];
            for attempt in 0..=delays.len() {
                match client
                    .purge_org_streams(&creds.o2_org, &creds.email_passcode)
                    .await
                {
                    Ok(streams) => {
                        tracing::info!(
                            org_id,
                            purged = streams.len(),
                            "O2 streams purged after org delete"
                        );
                        return;
                    }
                    Err(err) if attempt < delays.len() => {
                        tracing::warn!(org_id, attempt, %err, "O2 purge after org delete failed, retrying");
                        tokio::time::sleep(std::time::Duration::from_secs(delays[attempt])).await;
                    }
                    Err(err) => tracing::error!(
                        org_id,
                        o2_org = %creds.o2_org,
                        %err,
                        "O2 purge after org delete abandoned: purge this O2 organization manually"
                    ),
                }
            }
        });
    }
    Ok(StatusCode::NO_CONTENT.into_response())
}

// ────────────────────────────── Members ──────────────────────────────

async fn list_members_json(
    db: &sea_orm::DatabaseConnection,
    org_id: i64,
) -> Result<Vec<serde_json::Value>> {
    let members = organization_members::Entity::find()
        .filter(organization_members::Column::OrgId.eq(org_id))
        .find_also_related(users::Entity)
        .all(db)
        .await
        .map_err(|_| Error::InternalServerError)?;
    Ok(members
        .into_iter()
        .filter_map(|(m, u)| {
            u.map(|user| {
                serde_json::json!({
                    "user_id": user.id,
                    "email": user.email,
                    "full_name": user.full_name,
                    "role": api_role(m.role),
                    "created_at": m.created_at,
                })
            })
        })
        .collect())
}

#[derive(Debug, Default, Deserialize, utoipa::IntoParams)]
#[into_params(parameter_in = Query)]
struct MembersQuery {
    /// Case-insensitive search on email and full name.
    search: Option<String>,
    #[param(value_type = Option<i64>)]
    limit: Option<String>,
    #[param(value_type = Option<i64>)]
    offset: Option<String>,
}

/// `GET /api/v1/orgs/:id/members` — paginated (D14) + search on email/full name.
#[utoipa::path(
    get,
    path = "/api/v1/orgs/{id}/members",
    tag = "orgs",
    operation_id = "orgs_members_list",
    summary = "List the members of an organisation",
    description = "Paginated (`limit`/`offset`), optional search on email and full name. Any member (`orgs.members:read`); `X-Org-Id` must be the path org.",
    params(("id" = i64, Path, description = "Org id"), MembersQuery),
    responses(
        (status = 200, body = pnex_core::Paginated<pnex_core::OrgMember>),
        (status = 403, description = "`org-path-mismatch`: the path org is not the `X-Org-Id` one", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn members(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(org_id): Path<i64>,
    Query(q): Query<MembersQuery>,
) -> Result<Response> {
    same_org(&org, org_id)?;
    let page = pagination::PageParams::from(q.limit.as_deref(), q.offset.as_deref());
    let members = list_members_json(&ctx.db, org_id).await?;
    let filtered: Vec<serde_json::Value> = members
        .into_iter()
        .filter(|m| {
            pagination::rust_search_match(
                &q.search,
                &[
                    m["email"].as_str().unwrap_or_default(),
                    m["full_name"].as_str().unwrap_or_default(),
                ],
            )
        })
        .collect();
    let count = filtered.len() as i64;
    let (skip, take) = page.slice(filtered.len());
    let filters = q
        .search
        .map(|s| vec![("search".to_string(), s)])
        .unwrap_or_default();
    format::json(pagination::envelope(
        &format!("/api/v1/orgs/{org_id}/members"),
        &filters,
        page,
        count,
        filtered.into_iter().skip(skip).take(take).collect(),
    ))
}

#[derive(Deserialize)]
struct UpdateMemberParams {
    role: OrgRole,
}

/// `PATCH /api/v1/orgs/:id/members/:user_id` — role change.
/// Always at least one owner afterwards.
#[utoipa::path(
    patch,
    path = "/api/v1/orgs/{id}/members/{user_id}",
    tag = "orgs",
    operation_id = "orgs_members_update",
    summary = "Change the role of a member",
    description = "Needs `orgs.members:write` (owner or admin); touching an owner, or granting owner, also needs `orgs.owners:write` (owner). The org always keeps at least one owner. `X-Org-Id` must be the path org.",
    params(("id" = i64, Path, description = "Org id"), ("user_id" = i64, Path, description = "User id of the member")),
    request_body = pnex_core::UpdateMember,
    responses(
        (status = 200, body = MemberRoleUpdated),
        (status = 403, description = "`org-owner-edit-forbidden` (owner only), `org-path-mismatch`", body = crate::openapi::ErrorDetail),
        (status = 404, description = "Not a member of this org", body = crate::openapi::ErrorDetail),
        (status = 409, description = "Last owner (`org-last-owner`)", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn update_member(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path((org_id, user_id)): Path<(i64, i64)>,
    Json(params): Json<UpdateMemberParams>,
) -> Result<Response> {
    same_org(&org, org_id)?;
    let Some(target) = member_row(&ctx.db, org_id, user_id).await? else {
        return Err(Error::NotFound);
    };
    // Changing an owner (or promoting to the owner role): owner only.
    let touches_owner =
        matches!(target.role, OrgMemberRole::Owner) || matches!(params.role, OrgRole::Owner);
    if touches_owner && !org.can("orgs.owners:write") {
        return Err(forbidden(
            "org-owner-edit-forbidden",
            "Owner role required to modify an owner",
        ));
    }
    if matches!(target.role, OrgMemberRole::Owner) && !matches!(params.role, OrgRole::Owner) {
        last_owner_guard(&ctx.db, org_id).await?;
    }

    let mut active: organization_members::ActiveModel = target.into();
    active.role = Set(db_role(params.role));
    let updated = active
        .update(&ctx.db)
        .await
        .map_err(|_| Error::InternalServerError)?;
    format::json(MemberRoleUpdated {
        user_id: updated.user_id,
        role: api_role(updated.role),
    })
}

async fn member_row(
    db: &sea_orm::DatabaseConnection,
    org_id: i64,
    user_id: i64,
) -> Result<Option<organization_members::Model>> {
    organization_members::Entity::find()
        .filter(
            organization_members::Column::OrgId
                .eq(org_id)
                .and(organization_members::Column::UserId.eq(user_id)),
        )
        .one(db)
        .await
        .map_err(|_| Error::InternalServerError)
}

/// 409 when the org has one owner only (it is about to lose it).
async fn last_owner_guard(db: &sea_orm::DatabaseConnection, org_id: i64) -> Result<()> {
    let owners = organization_members::Entity::find()
        .filter(
            organization_members::Column::OrgId
                .eq(org_id)
                .and(organization_members::Column::Role.eq(OrgMemberRole::Owner)),
        )
        .count(db)
        .await
        .map_err(|_| Error::InternalServerError)?;
    if owners <= 1 {
        return Err(conflict(
            "org-last-owner",
            "The organization must keep at least one owner",
        ));
    }
    Ok(())
}

/// Removes a membership with what is private to it in the org.
async fn remove_membership(
    db: &sea_orm::DatabaseConnection,
    target: organization_members::Model,
) -> Result<()> {
    let (org_id, user_id) = (target.org_id, target.user_id);
    // Leaving an org erases the member's assistant conversations in it
    // (D145): they are private to the user within that org only. One
    // transaction: no membership removed with its conversations kept.
    let txn = db.begin().await.map_err(|_| Error::InternalServerError)?;
    organization_members::Entity::delete_by_id(target.id)
        .exec(&txn)
        .await
        .map_err(|_| Error::InternalServerError)?;
    crate::services::ai::conversations::delete_all(&txn, user_id, org_id)
        .await
        .map_err(|_| Error::InternalServerError)?;
    crate::services::extension::forget_member(&txn, org_id, user_id)
        .await
        .map_err(|_| Error::InternalServerError)?;
    // Their teams and roles in this org go too (iam.md D286).
    crate::services::iam::store::forget_member(&txn, org_id, user_id)
        .await
        .map_err(|_| Error::InternalServerError)?;
    txn.commit().await.map_err(|_| Error::InternalServerError)
}

/// `DELETE /api/v1/orgs/:id/members/:user_id` — removal.
#[utoipa::path(
    delete,
    path = "/api/v1/orgs/{id}/members/{user_id}",
    tag = "orgs",
    operation_id = "orgs_members_remove",
    summary = "Remove a member",
    description = "Needs `orgs.members:delete` (owner or admin); removing an owner also needs `orgs.owners:write` (owner) and never the last owner. Also deletes the member assistant conversations in this org. `X-Org-Id` must be the path org; leaving is `POST /api/v1/orgs/{id}/leave`.",
    params(("id" = i64, Path, description = "Org id"), ("user_id" = i64, Path, description = "User id of the member")),
    responses(
        (status = 204, description = "Removed"),
        (status = 403, description = "`org-owner-remove-forbidden` (owner only), `org-path-mismatch`", body = crate::openapi::ErrorDetail),
        (status = 404, description = "Not a member of this org", body = crate::openapi::ErrorDetail),
        (status = 409, description = "Last owner (`org-last-owner`)", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn remove_member(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path((org_id, user_id)): Path<(i64, i64)>,
) -> Result<Response> {
    same_org(&org, org_id)?;
    let Some(target) = member_row(&ctx.db, org_id, user_id).await? else {
        return Err(Error::NotFound);
    };
    if matches!(target.role, OrgMemberRole::Owner) {
        if !org.can("orgs.owners:write") {
            return Err(forbidden(
                "org-owner-remove-forbidden",
                "Owner role required to remove an owner",
            ));
        }
        last_owner_guard(&ctx.db, org_id).await?;
    }
    remove_membership(&ctx.db, target).await?;
    Ok(StatusCode::NO_CONTENT.into_response())
}

/// `POST /api/v1/orgs/:id/leave` — the caller leaves the org.
#[utoipa::path(
    post,
    path = "/api/v1/orgs/{id}/leave",
    tag = "orgs",
    operation_id = "orgs_leave",
    summary = "Leave an organisation",
    description = "The caller leaves the org, whatever their role, except its last owner. Deletes the caller's assistant conversations in this org. Not scoped by X-Org-Id.",
    params(("id" = i64, Path, description = "Org id")),
    responses(
        (status = 204, description = "Left"),
        (status = 404, description = "The caller is not a member of this org", body = crate::openapi::ErrorDetail),
        (status = 409, description = "Last owner (`org-last-owner`)", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = []))
)]
async fn leave(
    State(ctx): State<AppContext>,
    auth: AuthUser,
    Path(org_id): Path<i64>,
) -> Result<Response> {
    let Some((target, _)) = membership_of(&ctx.db, auth.user.id, org_id).await? else {
        return Err(Error::NotFound);
    };
    if matches!(target.role, OrgMemberRole::Owner) {
        last_owner_guard(&ctx.db, org_id).await?;
    }
    remove_membership(&ctx.db, target).await?;
    Ok(StatusCode::NO_CONTENT.into_response())
}

// ──────────────────────────── Invitations ────────────────────────────

#[derive(Deserialize)]
struct InviteParams {
    email: String,
    #[serde(default = "default_role")]
    role: OrgRole,
}
fn default_role() -> OrgRole {
    OrgRole::Viewer
}

fn invitation_json(row: &sea_orm::QueryResult) -> Result<pnex_core::OrgInvitation> {
    let role: String = row.try_get("", "role").map_err(db_error)?;
    let at = |col: &str| -> Result<String> {
        row.try_get::<sea_orm::prelude::DateTimeWithTimeZone>("", col)
            .map(|t| t.to_rfc3339())
            .map_err(db_error)
    };
    Ok(pnex_core::OrgInvitation {
        id: row.try_get("", "id").map_err(db_error)?,
        email: row.try_get("", "email").map_err(db_error)?,
        role: OrgRole::parse(&role).unwrap_or(OrgRole::Viewer),
        created_at: at("created_at")?,
        expires_at: at("expires_at")?,
    })
}

/// `POST /api/v1/orgs/:id/invitations` — invites an email.
#[utoipa::path(
    post,
    path = "/api/v1/orgs/{id}/invitations",
    tag = "orgs",
    operation_id = "orgs_invitations_create",
    summary = "Invite someone to an organisation",
    description = "Invites an email with a role (default viewer). Needs `orgs.members:write` (owner or admin); inviting as owner also needs `orgs.owners:write`. Nobody is added: the invited person accepts from their own session, with that email verified, within 14 days. The answer is the same whether an account uses that email or not; inviting the same email again replaces the pending invitation. `X-Org-Id` must be the path org.",
    params(("id" = i64, Path, description = "Org id")),
    request_body = pnex_core::InviteMember,
    responses(
        (status = 202, description = "Invitation pending", body = pnex_core::OrgInvitation),
        (status = 400, description = "`field-invalid`: `email` (`required`, `invalid`)", body = crate::openapi::ErrorDetail),
        (status = 403, description = "`org-owner-grant-forbidden` (owner only), `org-path-mismatch`", body = crate::openapi::ErrorDetail),
        (status = 409, description = "Already a member (`org-member-duplicate`)", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn invite(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(org_id): Path<i64>,
    Json(params): Json<InviteParams>,
) -> Result<Response> {
    same_org(&org, org_id)?;
    if matches!(params.role, OrgRole::Owner) && !org.can("orgs.owners:write") {
        return Err(forbidden(
            "org-owner-grant-forbidden",
            "Owner role required to grant the owner role",
        ));
    }
    let email = params.email.trim().to_lowercase();
    if email.is_empty() {
        return Ok(crate::controllers::field_error(
            "email",
            pnex_core::err_codes::FIELD_REQUIRED,
        ));
    }
    let shaped = email.len() <= 255
        && email
            .split_once('@')
            .is_some_and(|(local, domain)| !local.is_empty() && domain.contains('.'));
    if !shaped {
        return Ok(crate::controllers::field_error(
            "email",
            pnex_core::err_codes::FIELD_INVALID,
        ));
    }
    // Members are visible to whoever may invite: saying so leaks nothing.
    let member = ctx
        .db
        .query_one_raw(stmt(
            "SELECT 1 AS one FROM organization_members m JOIN users u ON u.id = m.user_id \
             WHERE m.org_id = $1 AND lower(u.email) = $2",
            vec![org_id.into(), email.clone().into()],
        ))
        .await
        .map_err(db_error)?;
    if member.is_some() {
        return Err(conflict(
            "org-member-duplicate",
            "User is already a member of this organization",
        ));
    }
    let row = ctx
        .db
        .query_one_raw(stmt(
            "INSERT INTO org_invitations (org_id, email, role, invited_by, expires_at) \
             VALUES ($1, $2, $3, $4, now() + make_interval(days => $5)) \
             ON CONFLICT (org_id, email) DO UPDATE SET role = EXCLUDED.role, \
               invited_by = EXCLUDED.invited_by, created_at = now(), expires_at = EXCLUDED.expires_at \
             RETURNING id, email, role, created_at, expires_at",
            vec![
                org_id.into(),
                email.into(),
                params.role.as_str().into(),
                org.auth.user.id.into(),
                INVITATION_TTL_DAYS.into(),
            ],
        ))
        .await
        .map_err(db_error)?
        .ok_or(Error::InternalServerError)?;
    Ok((StatusCode::ACCEPTED, format::json(invitation_json(&row)?)).into_response())
}

/// `GET /api/v1/orgs/:id/invitations` — pending invitations of the org.
#[utoipa::path(
    get,
    path = "/api/v1/orgs/{id}/invitations",
    tag = "orgs",
    operation_id = "orgs_invitations_list",
    summary = "List the pending invitations of an organisation",
    description = "Invitations not answered and not expired, newest first, paginated, optional search on the email. Needs `orgs.invitations:read` (owner or admin); `X-Org-Id` must be the path org.",
    params(("id" = i64, Path, description = "Org id"), MembersQuery),
    responses(
        (status = 200, body = pnex_core::Paginated<pnex_core::OrgInvitation>),
        (status = 403, description = "`org-path-mismatch`: the path org is not the `X-Org-Id` one", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn invitations(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(org_id): Path<i64>,
    Query(q): Query<MembersQuery>,
) -> Result<Response> {
    same_org(&org, org_id)?;
    let page = pagination::PageParams::from(q.limit.as_deref(), q.offset.as_deref());
    let rows = ctx
        .db
        .query_all_raw(stmt(
            "SELECT id, email, role, created_at, expires_at FROM org_invitations \
             WHERE org_id = $1 AND expires_at > now() ORDER BY created_at DESC, id DESC",
            vec![org_id.into()],
        ))
        .await
        .map_err(db_error)?;
    let mut all = Vec::with_capacity(rows.len());
    for row in &rows {
        let inv = invitation_json(row)?;
        if pagination::rust_search_match(&q.search, &[inv.email.as_str()]) {
            all.push(serde_json::to_value(inv).map_err(|_| Error::InternalServerError)?);
        }
    }
    let count = all.len() as i64;
    let (skip, take) = page.slice(all.len());
    let filters = q
        .search
        .map(|s| vec![("search".to_string(), s)])
        .unwrap_or_default();
    format::json(pagination::envelope(
        &format!("/api/v1/orgs/{org_id}/invitations"),
        &filters,
        page,
        count,
        all.into_iter().skip(skip).take(take).collect(),
    ))
}

fn invitation_not_found() -> Error {
    crate::controllers::coded_error(
        StatusCode::NOT_FOUND,
        pnex_core::err_codes::ORG_INVITATION_NOT_FOUND,
        "no pending invitation of yours under this id",
        None,
    )
}

/// `DELETE /api/v1/orgs/:id/invitations/:invitation_id` — revokes one.
#[utoipa::path(
    delete,
    path = "/api/v1/orgs/{id}/invitations/{invitation_id}",
    tag = "orgs",
    operation_id = "orgs_invitations_revoke",
    summary = "Revoke a pending invitation",
    description = "Withdraws an invitation before it is answered. Needs `orgs.members:delete` (owner or admin); `X-Org-Id` must be the path org.",
    params(("id" = i64, Path, description = "Org id"), ("invitation_id" = i64, Path, description = "Invitation id")),
    responses(
        (status = 204, description = "Revoked"),
        (status = 403, description = "`org-path-mismatch`: the path org is not the `X-Org-Id` one", body = crate::openapi::ErrorDetail),
        (status = 404, description = "No such invitation in this org (`org-invitation-not-found`)", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn revoke_invitation(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path((org_id, invitation_id)): Path<(i64, i64)>,
) -> Result<Response> {
    same_org(&org, org_id)?;
    let done = ctx
        .db
        .execute_raw(stmt(
            "DELETE FROM org_invitations WHERE id = $1 AND org_id = $2",
            vec![invitation_id.into(), org_id.into()],
        ))
        .await
        .map_err(db_error)?;
    if done.rows_affected() == 0 {
        return Err(invitation_not_found());
    }
    Ok(StatusCode::NO_CONTENT.into_response())
}

/// Email the caller may answer invitations for: a human whose identity
/// provider verified it (a service account never joins, iam.md §7).
fn invitee_email(auth: &AuthUser) -> Option<String> {
    if auth.service.is_some() || auth.claims.email_verified != Some(true) {
        return None;
    }
    let email = auth.claims.email.as_deref()?.trim().to_lowercase();
    (!email.is_empty()).then_some(email)
}

#[derive(Debug, Default, Deserialize, utoipa::IntoParams)]
#[into_params(parameter_in = Query)]
struct MyInvitationsQuery {
    #[param(value_type = Option<i64>)]
    limit: Option<String>,
    #[param(value_type = Option<i64>)]
    offset: Option<String>,
}

/// `GET /api/v1/invitations` — invitations waiting for my answer.
#[utoipa::path(
    get,
    path = "/api/v1/invitations",
    tag = "orgs",
    operation_id = "invitations_list",
    summary = "List my pending invitations",
    description = "Invitations addressed to the caller's email, not expired, for orgs the caller is not a member of yet, newest first, paginated. Empty while the identity provider has not verified the email. Not scoped by X-Org-Id.",
    params(MyInvitationsQuery),
    responses((status = 200, body = pnex_core::Paginated<pnex_core::MyInvitation>)),
    security(("bearer" = []))
)]
async fn my_invitations(
    State(ctx): State<AppContext>,
    auth: AuthUser,
    Query(q): Query<MyInvitationsQuery>,
) -> Result<Response> {
    let page = pagination::PageParams::from(q.limit.as_deref(), q.offset.as_deref());
    let mut all = Vec::new();
    if let Some(email) = invitee_email(&auth) {
        let rows = ctx
            .db
            .query_all_raw(stmt(
                "SELECT i.id, i.org_id, o.name AS org_name, i.role, i.expires_at, \
                   coalesce(nullif(u.full_name, ''), u.email) AS invited_by \
                 FROM org_invitations i JOIN organizations o ON o.id = i.org_id \
                 LEFT JOIN users u ON u.id = i.invited_by \
                 WHERE i.email = $1 AND i.expires_at > now() AND NOT EXISTS ( \
                   SELECT 1 FROM organization_members m WHERE m.org_id = i.org_id AND m.user_id = $2) \
                 ORDER BY i.created_at DESC, i.id DESC",
                vec![email.into(), auth.user.id.into()],
            ))
            .await
            .map_err(db_error)?;
        for row in &rows {
            let role: String = row.try_get("", "role").map_err(db_error)?;
            let expires: sea_orm::prelude::DateTimeWithTimeZone =
                row.try_get("", "expires_at").map_err(db_error)?;
            let inv = pnex_core::MyInvitation {
                id: row.try_get("", "id").map_err(db_error)?,
                org_id: row.try_get("", "org_id").map_err(db_error)?,
                org_name: row.try_get("", "org_name").map_err(db_error)?,
                role: OrgRole::parse(&role).unwrap_or(OrgRole::Viewer),
                invited_by: row.try_get("", "invited_by").map_err(db_error)?,
                expires_at: expires.to_rfc3339(),
            };
            all.push(serde_json::to_value(inv).map_err(|_| Error::InternalServerError)?);
        }
    }
    let count = all.len() as i64;
    let (skip, take) = page.slice(all.len());
    format::json(pagination::envelope(
        "/api/v1/invitations",
        &[],
        page,
        count,
        all.into_iter().skip(skip).take(take).collect(),
    ))
}

/// `POST /api/v1/invitations/:id/accept` — joins the org with the invited role.
#[utoipa::path(
    post,
    path = "/api/v1/invitations/{id}/accept",
    tag = "orgs",
    operation_id = "invitations_accept",
    summary = "Accept an invitation",
    description = "The caller joins the org with the invited role and the invitation is used up. Only for the invited email, verified by the identity provider, before expiry. Not scoped by X-Org-Id.",
    params(("id" = i64, Path, description = "Invitation id")),
    responses(
        (status = 200, description = "Joined", body = OrgCreated),
        (status = 404, description = "No pending invitation of the caller under this id (`org-invitation-not-found`)", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = []))
)]
async fn accept_invitation(
    State(ctx): State<AppContext>,
    auth: AuthUser,
    Path(id): Path<i64>,
) -> Result<Response> {
    let email = invitee_email(&auth).ok_or_else(invitation_not_found)?;
    let txn = ctx.db.begin().await.map_err(db_error)?;
    let row = txn
        .query_one_raw(stmt(
            "DELETE FROM org_invitations WHERE id = $1 AND email = $2 AND expires_at > now() \
             RETURNING org_id, role",
            vec![id.into(), email.into()],
        ))
        .await
        .map_err(db_error)?
        .ok_or_else(invitation_not_found)?;
    let org_id: i64 = row.try_get("", "org_id").map_err(db_error)?;
    let role: String = row.try_get("", "role").map_err(db_error)?;
    let role = OrgRole::parse(&role).unwrap_or(OrgRole::Viewer);
    let org = organizations::Entity::find_by_id(org_id)
        .one(&txn)
        .await
        .map_err(db_error)?
        .ok_or_else(invitation_not_found)?;
    let existing = organization_members::Entity::find()
        .filter(
            organization_members::Column::OrgId
                .eq(org_id)
                .and(organization_members::Column::UserId.eq(auth.user.id)),
        )
        .one(&txn)
        .await
        .map_err(db_error)?;
    // Already a member (joined another way meanwhile): the role in place stays.
    let role = match existing {
        Some(m) => api_role(m.role),
        None => {
            organization_members::ActiveModel {
                org_id: Set(org_id),
                user_id: Set(auth.user.id),
                role: Set(db_role(role)),
                ..Default::default()
            }
            .insert(&txn)
            .await
            .map_err(db_error)?;
            role
        }
    };
    txn.commit().await.map_err(db_error)?;
    format::json(OrgCreated {
        id: org.id,
        name: org.name,
        role,
    })
}

/// `POST /api/v1/invitations/:id/decline` — refuses an invitation.
#[utoipa::path(
    post,
    path = "/api/v1/invitations/{id}/decline",
    tag = "orgs",
    operation_id = "invitations_decline",
    summary = "Decline an invitation",
    description = "The invitation addressed to the caller's verified email is dropped; the org can invite again. Not scoped by X-Org-Id.",
    params(("id" = i64, Path, description = "Invitation id")),
    responses(
        (status = 204, description = "Declined"),
        (status = 404, description = "No pending invitation of the caller under this id (`org-invitation-not-found`)", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = []))
)]
async fn decline_invitation(
    State(ctx): State<AppContext>,
    auth: AuthUser,
    Path(id): Path<i64>,
) -> Result<Response> {
    let email = invitee_email(&auth).ok_or_else(invitation_not_found)?;
    let done = ctx
        .db
        .execute_raw(stmt(
            "DELETE FROM org_invitations WHERE id = $1 AND email = $2",
            vec![id.into(), email.into()],
        ))
        .await
        .map_err(db_error)?;
    if done.rows_affected() == 0 {
        return Err(invitation_not_found());
    }
    Ok(StatusCode::NO_CONTENT.into_response())
}

pub fn routes() -> Routes {
    Routes::new()
        .prefix("/api/v1")
        .add("/orgs", get(list).post(create))
        .add("/orgs/{id}", get(detail).patch(update).delete(delete))
        .add("/orgs/{id}/leave", post(leave))
        .add("/orgs/{id}/members", get(members))
        .add(
            "/orgs/{id}/members/{user_id}",
            patch(update_member).delete(remove_member),
        )
        .add("/orgs/{id}/invitations", get(invitations).post(invite))
        .add(
            "/orgs/{id}/invitations/{invitation_id}",
            axum::routing::delete(revoke_invitation),
        )
        .add("/invitations", get(my_invitations))
        .add("/invitations/{id}/accept", post(accept_invitation))
        .add("/invitations/{id}/decline", post(decline_invitation))
}

/// OpenAPI paths of this controller (merged by `crate::openapi::spec`).
pub fn openapi() -> utoipa::openapi::OpenApi {
    #[derive(utoipa::OpenApi)]
    #[openapi(
        paths(
            list,
            create,
            detail,
            update,
            delete,
            leave,
            members,
            update_member,
            remove_member,
            invite,
            invitations,
            revoke_invitation,
            my_invitations,
            accept_invitation,
            decline_invitation
        ),
        components(schemas(pnex_core::InviteMember))
    )]
    struct Doc;
    <Doc as utoipa::OpenApi>::openapi()
}
