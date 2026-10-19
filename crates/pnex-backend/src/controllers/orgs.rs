//! Organizations and their members management (multi-tenant, decision D2).
//!
//! Règles d'accès :
//! - lecture (org, membres) : tout membre ;
//! - écriture (rename, ajout/modif/retrait de membre) : owner ou admin ;
//! - suppression d'org, gestion des owners : owner uniquement ;
//! - il doit toujours rester au moins un owner par org ;
//! - suppression d'org : owner et dernier membre (les données partent en
//!   cascade — on force un retrait explicite des autres membres avant).
//!
//! Ajout de membre : par email d'un utilisateur **déjà provisionné** (il doit
//! s'être connecté au moins une fois). Les invitations par email à un
//! utilisateur inexistant attendent l'infrastructure SMTP (phase ultérieure).

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use loco_rs::prelude::*;
use pnex_core::OrgRole;
use sea_orm::TransactionTrait;
use sea_orm::{
    ActiveModelTrait, ColumnTrait, EntityTrait, ExprTrait, PaginatorTrait, QueryFilter, Set,
};
use serde::Deserialize;

use super::pagination;
use crate::auth::AuthUser;
use crate::models::_entities::{
    organization_members, organizations, sea_orm_active_enums::OrgMemberRole, subscription_tiers,
    users,
};

/// Membership + org pour (user, org), si l'utilisateur en est membre.
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
    // find_also_related sur une FK obligatoire : l'org existe toujours.
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

/// Org governance (settings, members): owner or admin, never member.
fn can_administer(role: OrgMemberRole) -> bool {
    matches!(role, OrgMemberRole::Owner | OrgMemberRole::Admin)
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

/// Answer of `POST /api/v1/orgs` (shaped like `OrgSummary`).
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

/// Answer of `POST /api/v1/orgs/{id}/members`.
#[derive(serde::Serialize, utoipa::ToSchema)]
struct MemberAdded {
    user_id: i64,
    email: String,
    role: OrgRole,
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

/// Tier d'une nouvelle org — `PNEX_DEFAULT_ORG_TIER` (dev : « Admin » pour
/// débrider le compte de test), Free sinon ; fallback Free si le tier nommé
/// n'existe pas (jamais d'org sans tier à cause d'une coquille de config).
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

/// `GET /api/v1/orgs` — orgs dont je suis membre (avec rôle et tier),
/// paginées (D14) + recherche sur le nom.
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

    // L'ensemble (orgs d'un user) est borné par nature : filtre Rust puis
    // découpage — le count reflète le total filtré.
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

/// `POST /api/v1/orgs` — création, le créateur devient owner (tier Free).
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

/// `GET /api/v1/orgs/:id` — détail (membres inclus), membres uniquement.
#[utoipa::path(
    get,
    path = "/api/v1/orgs/{id}",
    tag = "orgs",
    operation_id = "orgs_get",
    summary = "Get an organisation with its members",
    description = "Org detail, the caller role and the full member list. Members only (404 otherwise).",
    params(("id" = i64, Path, description = "Org id")),
    responses(
        (status = 200, body = pnex_core::OrgDetail),
        (status = 404, description = "Unknown org or caller not a member", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = []))
)]
async fn detail(
    State(ctx): State<AppContext>,
    auth: AuthUser,
    Path(org_id): Path<i64>,
) -> Result<Response> {
    let Some((membership, org)) = membership_of(&ctx.db, auth.user.id, org_id).await? else {
        return Err(Error::NotFound);
    };
    let members = list_members_json(&ctx.db, org_id).await?;
    format::json(serde_json::json!({
        "id": org.id,
        "name": org.name,
        "subscription_tier_id": org.subscription_tier_id,
        "role": api_role(membership.role),
        "members": members,
    }))
}

#[derive(Deserialize)]
struct UpdateOrgParams {
    name: String,
}

/// `PATCH /api/v1/orgs/:id` — renommage (owner/admin).
#[utoipa::path(
    patch,
    path = "/api/v1/orgs/{id}",
    tag = "orgs",
    operation_id = "orgs_update",
    summary = "Rename an organisation",
    description = "Renames the org. Owner or admin only. Names are globally unique.",
    params(("id" = i64, Path, description = "Org id")),
    request_body = pnex_core::UpdateOrg,
    responses(
        (status = 200, body = OrgRenamed),
        (status = 400, description = "`field-invalid`: empty name (`errors.fields.name = required`)", body = crate::openapi::ErrorDetail),
        (status = 403, description = "Not owner/admin (`org-rename-forbidden`)", body = crate::openapi::ErrorDetail),
        (status = 404, description = "Unknown org or caller not a member", body = crate::openapi::ErrorDetail),
        (status = 409, description = "Name already used (`org-name-duplicate`)", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = []))
)]
async fn update(
    State(ctx): State<AppContext>,
    auth: AuthUser,
    Path(org_id): Path<i64>,
    Json(params): Json<UpdateOrgParams>,
) -> Result<Response> {
    let Some((membership, org)) = membership_of(&ctx.db, auth.user.id, org_id).await? else {
        return Err(Error::NotFound);
    };
    if !can_administer(membership.role) {
        return Err(forbidden(
            "org-rename-forbidden",
            "Owner or admin role required to rename an organization",
        ));
    }
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

    let mut active: organizations::ActiveModel = org.into();
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

/// `DELETE /api/v1/orgs/:id` — owner et dernier membre uniquement.
#[utoipa::path(
    delete,
    path = "/api/v1/orgs/{id}",
    tag = "orgs",
    operation_id = "orgs_delete",
    summary = "Delete an organisation",
    description = "Deletes the org and, by cascade, all its data. Owner only, and only when the owner is the last member. Telemetry streams are purged from OpenObserve in the background (best effort).",
    params(("id" = i64, Path, description = "Org id")),
    responses(
        (status = 204, description = "Deleted"),
        (status = 403, description = "Not owner (`org-delete-forbidden`)", body = crate::openapi::ErrorDetail),
        (status = 404, description = "Unknown org or caller not a member", body = crate::openapi::ErrorDetail),
        (status = 409, description = "Other members remain (`org-not-empty`)", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = []))
)]
async fn delete(
    State(ctx): State<AppContext>,
    auth: AuthUser,
    Path(org_id): Path<i64>,
) -> Result<Response> {
    let Some((membership, org)) = membership_of(&ctx.db, auth.user.id, org_id).await? else {
        return Err(Error::NotFound);
    };
    if !matches!(membership.role, OrgMemberRole::Owner) {
        return Err(forbidden(
            "org-delete-forbidden",
            "Owner role required to delete the organization",
        ));
    }
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
                match client.purge_metric_streams(&creds.o2_org).await {
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

// ────────────────────────────── Membres ──────────────────────────────

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

/// `GET /api/v1/orgs/:id/members` — membres uniquement, paginés (D14) +
/// recherche sur email/nom complet.
#[utoipa::path(
    get,
    path = "/api/v1/orgs/{id}/members",
    tag = "orgs",
    operation_id = "orgs_members_list",
    summary = "List the members of an organisation",
    description = "Paginated (`limit`/`offset`), optional search on email and full name. Members only.",
    params(("id" = i64, Path, description = "Org id"), MembersQuery),
    responses(
        (status = 200, body = pnex_core::Paginated<pnex_core::OrgMember>),
        (status = 404, description = "Unknown org or caller not a member", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = []))
)]
async fn members(
    State(ctx): State<AppContext>,
    auth: AuthUser,
    Path(org_id): Path<i64>,
    Query(q): Query<MembersQuery>,
) -> Result<Response> {
    if membership_of(&ctx.db, auth.user.id, org_id)
        .await?
        .is_none()
    {
        return Err(Error::NotFound);
    }
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
struct AddMemberParams {
    email: String,
    #[serde(default = "default_role")]
    role: OrgRole,
}
fn default_role() -> OrgRole {
    OrgRole::Viewer
}

/// `POST /api/v1/orgs/:id/members` — ajout d'un utilisateur déjà provisionné
/// (owner/admin). Promouvoir au rôle owner : owner uniquement.
#[utoipa::path(
    post,
    path = "/api/v1/orgs/{id}/members",
    tag = "orgs",
    operation_id = "orgs_members_add",
    summary = "Add a member to an organisation",
    description = "Adds an already provisioned user (they must have signed in once) by email. Owner or admin; granting the owner role requires owner. Default role viewer.",
    params(("id" = i64, Path, description = "Org id")),
    request_body = pnex_core::AddMember,
    responses(
        (status = 201, body = MemberAdded),
        (status = 403, description = "Not allowed (`org-member-add-forbidden`, `org-owner-grant-forbidden`)", body = crate::openapi::ErrorDetail),
        (status = 404, description = "Unknown org, caller not a member, or unknown user (`org-member-unknown-user`)", body = crate::openapi::ErrorDetail),
        (status = 409, description = "Already a member (`org-member-duplicate`)", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = []))
)]
async fn add_member(
    State(ctx): State<AppContext>,
    auth: AuthUser,
    Path(org_id): Path<i64>,
    Json(params): Json<AddMemberParams>,
) -> Result<Response> {
    let Some((membership, _org)) = membership_of(&ctx.db, auth.user.id, org_id).await? else {
        return Err(Error::NotFound);
    };
    if !can_administer(membership.role) {
        return Err(forbidden(
            "org-member-add-forbidden",
            "Owner or admin role required to add a member",
        ));
    }
    if matches!(params.role, OrgRole::Owner) && !matches!(membership.role, OrgMemberRole::Owner) {
        return Err(forbidden(
            "org-owner-grant-forbidden",
            "Owner role required to grant the owner role",
        ));
    }

    // A service account is never a member (iam.md §7): unknown here.
    let Some(target) = users::Entity::find()
        .filter(users::Column::Email.eq(params.email.trim()))
        .filter(users::Column::IsService.eq(false))
        .one(&ctx.db)
        .await
        .map_err(|_| Error::InternalServerError)?
    else {
        return Err(Error::CustomError(
            StatusCode::NOT_FOUND,
            loco_rs::controller::ErrorDetail::new(
                "org-member-unknown-user",
                "This user does not exist yet — they must sign in at least once before being added"
                    .to_string(),
            ),
        ));
    };
    if organization_members::Entity::find()
        .filter(
            organization_members::Column::OrgId
                .eq(org_id)
                .and(organization_members::Column::UserId.eq(target.id)),
        )
        .one(&ctx.db)
        .await
        .map_err(|_| Error::InternalServerError)?
        .is_some()
    {
        return Err(conflict(
            "org-member-duplicate",
            "User is already a member of this organization",
        ));
    }

    organization_members::ActiveModel {
        org_id: Set(org_id),
        user_id: Set(target.id),
        role: Set(db_role(params.role)),
        ..Default::default()
    }
    .insert(&ctx.db)
    .await
    .map_err(|_| Error::InternalServerError)?;

    Ok((
        StatusCode::CREATED,
        format::json(MemberAdded {
            user_id: target.id,
            email: target.email,
            role: params.role,
        }),
    )
        .into_response())
}

#[derive(Deserialize)]
struct UpdateMemberParams {
    role: OrgRole,
}

/// `PATCH /api/v1/orgs/:id/members/:user_id` — changement de rôle.
/// Toujours au moins un owner en sortie.
#[utoipa::path(
    patch,
    path = "/api/v1/orgs/{id}/members/{user_id}",
    tag = "orgs",
    operation_id = "orgs_members_update",
    summary = "Change the role of a member",
    description = "Owner or admin; touching an owner (or granting owner) requires owner. The org always keeps at least one owner.",
    params(("id" = i64, Path, description = "Org id"), ("user_id" = i64, Path, description = "User id of the member")),
    request_body = pnex_core::UpdateMember,
    responses(
        (status = 200, body = MemberRoleUpdated),
        (status = 403, description = "Not allowed (`org-member-role-forbidden`, `org-owner-edit-forbidden`)", body = crate::openapi::ErrorDetail),
        (status = 404, description = "Unknown org/member or caller not a member", body = crate::openapi::ErrorDetail),
        (status = 409, description = "Last owner (`org-last-owner`)", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = []))
)]
async fn update_member(
    State(ctx): State<AppContext>,
    auth: AuthUser,
    Path((org_id, user_id)): Path<(i64, i64)>,
    Json(params): Json<UpdateMemberParams>,
) -> Result<Response> {
    let Some((membership, _org)) = membership_of(&ctx.db, auth.user.id, org_id).await? else {
        return Err(Error::NotFound);
    };
    let Some(target) = organization_members::Entity::find()
        .filter(
            organization_members::Column::OrgId
                .eq(org_id)
                .and(organization_members::Column::UserId.eq(user_id)),
        )
        .one(&ctx.db)
        .await
        .map_err(|_| Error::InternalServerError)?
    else {
        return Err(Error::NotFound);
    };

    if !can_administer(membership.role) {
        return Err(forbidden(
            "org-member-role-forbidden",
            "Owner or admin role required to change a member role",
        ));
    }
    // Modifier un owner (ou promouvoir au rôle owner) : owner uniquement.
    let touches_owner =
        matches!(target.role, OrgMemberRole::Owner) || matches!(params.role, OrgRole::Owner);
    if touches_owner && !matches!(membership.role, OrgMemberRole::Owner) {
        return Err(forbidden(
            "org-owner-edit-forbidden",
            "Owner role required to modify an owner",
        ));
    }
    // Garde : au moins un owner reste.
    if matches!(target.role, OrgMemberRole::Owner) && !matches!(params.role, OrgRole::Owner) {
        let owners = organization_members::Entity::find()
            .filter(
                organization_members::Column::OrgId
                    .eq(org_id)
                    .and(organization_members::Column::Role.eq(OrgMemberRole::Owner)),
            )
            .count(&ctx.db)
            .await
            .map_err(|_| Error::InternalServerError)?;
        if owners <= 1 {
            return Err(conflict(
                "org-last-owner",
                "The organization must keep at least one owner",
            ));
        }
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

/// `DELETE /api/v1/orgs/:id/members/:user_id` — retrait (ou départ volontaire).
#[utoipa::path(
    delete,
    path = "/api/v1/orgs/{id}/members/{user_id}",
    tag = "orgs",
    operation_id = "orgs_members_remove",
    summary = "Remove a member (or leave)",
    description = "Removes a member, or lets the caller leave. Removing someone else needs owner or admin; removing an owner needs owner and never the last owner. Also deletes the member assistant conversations in this org.",
    params(("id" = i64, Path, description = "Org id"), ("user_id" = i64, Path, description = "User id of the member")),
    responses(
        (status = 204, description = "Removed"),
        (status = 403, description = "Not allowed (`org-member-remove-forbidden`, `org-owner-remove-forbidden`)", body = crate::openapi::ErrorDetail),
        (status = 404, description = "Unknown org/member or caller not a member", body = crate::openapi::ErrorDetail),
        (status = 409, description = "Last owner (`org-last-owner`)", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = []))
)]
async fn remove_member(
    State(ctx): State<AppContext>,
    auth: AuthUser,
    Path((org_id, user_id)): Path<(i64, i64)>,
) -> Result<Response> {
    let Some((membership, _org)) = membership_of(&ctx.db, auth.user.id, org_id).await? else {
        return Err(Error::NotFound);
    };
    let Some(target) = organization_members::Entity::find()
        .filter(
            organization_members::Column::OrgId
                .eq(org_id)
                .and(organization_members::Column::UserId.eq(user_id)),
        )
        .one(&ctx.db)
        .await
        .map_err(|_| Error::InternalServerError)?
    else {
        return Err(Error::NotFound);
    };

    let self_removal = user_id == auth.user.id;
    if !self_removal && !can_administer(membership.role) {
        return Err(forbidden(
            "org-member-remove-forbidden",
            "Owner or admin role required to remove a member",
        ));
    }
    if matches!(target.role, OrgMemberRole::Owner) {
        // Retirer un owner (même soi-même) : owner uniquement.
        if !matches!(membership.role, OrgMemberRole::Owner) {
            return Err(forbidden(
                "org-owner-remove-forbidden",
                "Owner role required to remove an owner",
            ));
        }
        let owners = organization_members::Entity::find()
            .filter(
                organization_members::Column::OrgId
                    .eq(org_id)
                    .and(organization_members::Column::Role.eq(OrgMemberRole::Owner)),
            )
            .count(&ctx.db)
            .await
            .map_err(|_| Error::InternalServerError)?;
        if owners <= 1 {
            return Err(conflict(
                "org-last-owner",
                "The organization must keep at least one owner",
            ));
        }
    }

    // Leaving an org erases the member's assistant conversations in it
    // (D145): they are private to the user within that org only. One
    // transaction: no membership removed with its conversations kept.
    let txn = ctx
        .db
        .begin()
        .await
        .map_err(|_| Error::InternalServerError)?;
    organization_members::Entity::delete_by_id(target.id)
        .exec(&txn)
        .await
        .map_err(|_| Error::InternalServerError)?;
    crate::services::ai::conversations::delete_all(&txn, user_id, org_id)
        .await
        .map_err(|_| Error::InternalServerError)?;
    // Their teams and roles in this org go too (iam.md D286).
    crate::services::iam::store::forget_member(&txn, org_id, user_id)
        .await
        .map_err(|_| Error::InternalServerError)?;
    txn.commit().await.map_err(|_| Error::InternalServerError)?;
    Ok(StatusCode::NO_CONTENT.into_response())
}

pub fn routes() -> Routes {
    Routes::new()
        .prefix("/api/v1/orgs")
        .add("", get(list).post(create))
        .add("/{id}", get(detail).patch(update).delete(delete))
        .add("/{id}/members", get(members).post(add_member))
        .add(
            "/{id}/members/{user_id}",
            patch(update_member).delete(remove_member),
        )
}

/// OpenAPI paths of this controller (merged by `crate::openapi::spec`).
pub fn openapi() -> utoipa::openapi::OpenApi {
    #[derive(utoipa::OpenApi)]
    #[openapi(paths(
        list,
        create,
        detail,
        update,
        delete,
        members,
        add_member,
        update_member,
        remove_member
    ))]
    struct Doc;
    <Doc as utoipa::OpenApi>::openapi()
}
