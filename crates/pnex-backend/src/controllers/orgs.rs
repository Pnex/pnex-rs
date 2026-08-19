//! Gestion des organisations et de leurs membres — API nouvelle (concept
//! multi-tenant absent du Django POC, décision D2 validée).
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

fn forbidden(msg: &str) -> Error {
    Error::CustomError(
        StatusCode::FORBIDDEN,
        loco_rs::controller::ErrorDetail::new("forbidden", msg.to_string()),
    )
}

fn conflict(msg: &str) -> Error {
    Error::CustomError(
        StatusCode::CONFLICT,
        loco_rs::controller::ErrorDetail::new("conflict", msg.to_string()),
    )
}

fn can_write(role: OrgMemberRole) -> bool {
    matches!(role, OrgMemberRole::Owner | OrgMemberRole::Admin)
}

/// Rôle tel qu'exposé dans l'API : minuscules (« owner », « admin », « viewer »).
pub fn role_str(role: OrgMemberRole) -> &'static str {
    match role {
        OrgMemberRole::Owner => "owner",
        OrgMemberRole::Admin => "admin",
        OrgMemberRole::Viewer => "viewer",
    }
}

/// Rôle accepté en entrée d'API (minuscules).
#[derive(Clone, Copy, Debug, Deserialize)]
#[serde(rename_all = "lowercase")]
enum RoleParam {
    Owner,
    Admin,
    Viewer,
}

impl From<RoleParam> for OrgMemberRole {
    fn from(value: RoleParam) -> Self {
        match value {
            RoleParam::Owner => OrgMemberRole::Owner,
            RoleParam::Admin => OrgMemberRole::Admin,
            RoleParam::Viewer => OrgMemberRole::Viewer,
        }
    }
}

// ─────────────────────────────── Orgs ───────────────────────────────

#[derive(Debug, Default, Deserialize)]
struct ListOrgsQuery {
    /// Recherche OU sur le nom de l'org.
    search: Option<String>,
    limit: Option<String>,
    offset: Option<String>,
}

/// `GET /api/v1/orgs` — orgs dont je suis membre (avec rôle et tier),
/// paginées (D14) + recherche sur le nom.
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
                "role": role_str(m.role),
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
async fn create(
    State(ctx): State<AppContext>,
    auth: AuthUser,
    Json(params): Json<CreateOrgParams>,
) -> Result<Response> {
    let name = params.name.trim().to_string();
    if name.is_empty() {
        return Err(Error::BadRequest("name requis".into()));
    }
    if organizations::Entity::find()
        .filter(organizations::Column::Name.eq(&name))
        .one(&ctx.db)
        .await
        .map_err(|_| Error::InternalServerError)?
        .is_some()
    {
        return Err(conflict("une organisation porte déjà ce nom"));
    }

    let free_tier = subscription_tiers::Entity::find()
        .filter(subscription_tiers::Column::Name.eq("Free"))
        .one(&ctx.db)
        .await
        .map_err(|_| Error::InternalServerError)?;

    let org = organizations::ActiveModel {
        name: Set(name),
        subscription_tier_id: Set(free_tier.map(|t| t.id)),
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
        format::json(serde_json::json!({ "id": org.id, "name": org.name })),
    )
        .into_response())
}

/// `GET /api/v1/orgs/:id` — détail (membres inclus), membres uniquement.
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
        "role": role_str(membership.role),
        "members": members,
    }))
}

#[derive(Deserialize)]
struct UpdateOrgParams {
    name: String,
}

/// `PATCH /api/v1/orgs/:id` — renommage (owner/admin).
async fn update(
    State(ctx): State<AppContext>,
    auth: AuthUser,
    Path(org_id): Path<i64>,
    Json(params): Json<UpdateOrgParams>,
) -> Result<Response> {
    let Some((membership, org)) = membership_of(&ctx.db, auth.user.id, org_id).await? else {
        return Err(Error::NotFound);
    };
    if !can_write(membership.role) {
        return Err(forbidden("owner ou admin requis pour renommer"));
    }
    let name = params.name.trim().to_string();
    if name.is_empty() {
        return Err(Error::BadRequest("name requis".into()));
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
        return Err(conflict("une organisation porte déjà ce nom"));
    }

    let mut active: organizations::ActiveModel = org.into();
    active.name = Set(name);
    let org = active
        .update(&ctx.db)
        .await
        .map_err(|_| Error::InternalServerError)?;
    format::json(serde_json::json!({ "id": org.id, "name": org.name }))
}

/// `DELETE /api/v1/orgs/:id` — owner et dernier membre uniquement.
async fn delete(
    State(ctx): State<AppContext>,
    auth: AuthUser,
    Path(org_id): Path<i64>,
) -> Result<Response> {
    let Some((membership, org)) = membership_of(&ctx.db, auth.user.id, org_id).await? else {
        return Err(Error::NotFound);
    };
    if !matches!(membership.role, OrgMemberRole::Owner) {
        return Err(forbidden("owner requis pour supprimer l'organisation"));
    }
    let member_count = organization_members::Entity::find()
        .filter(organization_members::Column::OrgId.eq(org_id))
        .count(&ctx.db)
        .await
        .map_err(|_| Error::InternalServerError)?;
    if member_count > 1 {
        return Err(conflict(
            "l'organisation doit être vide (retirez les autres membres d'abord)",
        ));
    }

    organizations::Entity::delete_by_id(org.id)
        .exec(&ctx.db)
        .await
        .map_err(|_| Error::InternalServerError)?;
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
                    "role": role_str(m.role),
                    "created_at": m.created_at,
                })
            })
        })
        .collect())
}

#[derive(Debug, Default, Deserialize)]
struct MembersQuery {
    /// Recherche OU sur email et nom complet.
    search: Option<String>,
    limit: Option<String>,
    offset: Option<String>,
}

/// `GET /api/v1/orgs/:id/members` — membres uniquement, paginés (D14) +
/// recherche sur email/nom complet.
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
    role: RoleParam,
}
fn default_role() -> RoleParam {
    RoleParam::Viewer
}

/// `POST /api/v1/orgs/:id/members` — ajout d'un utilisateur déjà provisionné
/// (owner/admin). Promouvoir au rôle owner : owner uniquement.
async fn add_member(
    State(ctx): State<AppContext>,
    auth: AuthUser,
    Path(org_id): Path<i64>,
    Json(params): Json<AddMemberParams>,
) -> Result<Response> {
    let Some((membership, _org)) = membership_of(&ctx.db, auth.user.id, org_id).await? else {
        return Err(Error::NotFound);
    };
    if !can_write(membership.role) {
        return Err(forbidden("owner ou admin requis pour ajouter un membre"));
    }
    if matches!(params.role, RoleParam::Owner) && !matches!(membership.role, OrgMemberRole::Owner) {
        return Err(forbidden("owner requis pour promouvoir au rôle owner"));
    }

    let Some(target) = users::Entity::find()
        .filter(users::Column::Email.eq(params.email.trim()))
        .one(&ctx.db)
        .await
        .map_err(|_| Error::InternalServerError)?
    else {
        return Err(Error::CustomError(
            StatusCode::NOT_FOUND,
            loco_rs::controller::ErrorDetail::new(
                "user_unknown",
                "cet utilisateur n'existe pas encore — il doit d'abord se connecter au moins une fois".to_string(),
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
        return Err(conflict("déjà membre de l'organisation"));
    }

    organization_members::ActiveModel {
        org_id: Set(org_id),
        user_id: Set(target.id),
        role: Set(OrgMemberRole::from(params.role)),
        ..Default::default()
    }
    .insert(&ctx.db)
    .await
    .map_err(|_| Error::InternalServerError)?;

    Ok((
        StatusCode::CREATED,
        format::json(serde_json::json!({
            "user_id": target.id, "email": target.email, "role": role_str(OrgMemberRole::from(params.role)),
        })),
    )
        .into_response())
}

#[derive(Deserialize)]
struct UpdateMemberParams {
    role: RoleParam,
}

/// `PATCH /api/v1/orgs/:id/members/:user_id` — changement de rôle.
/// Toujours au moins un owner en sortie.
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

    if !can_write(membership.role) {
        return Err(forbidden("owner ou admin requis pour modifier un rôle"));
    }
    // Modifier un owner (ou promouvoir au rôle owner) : owner uniquement.
    let touches_owner =
        matches!(target.role, OrgMemberRole::Owner) || matches!(params.role, RoleParam::Owner);
    if touches_owner && !matches!(membership.role, OrgMemberRole::Owner) {
        return Err(forbidden("owner requis pour modifier un owner"));
    }
    // Garde : au moins un owner reste.
    if matches!(target.role, OrgMemberRole::Owner) && !matches!(params.role, RoleParam::Owner) {
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
            return Err(conflict("l'organisation doit garder au moins un owner"));
        }
    }

    let mut active: organization_members::ActiveModel = target.into();
    active.role = Set(OrgMemberRole::from(params.role));
    let updated = active
        .update(&ctx.db)
        .await
        .map_err(|_| Error::InternalServerError)?;
    format::json(serde_json::json!({
        "user_id": updated.user_id, "role": updated.role,
    }))
}

/// `DELETE /api/v1/orgs/:id/members/:user_id` — retrait (ou départ volontaire).
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
    if !self_removal && !can_write(membership.role) {
        return Err(forbidden("owner ou admin requis pour retirer un membre"));
    }
    if matches!(target.role, OrgMemberRole::Owner) {
        // Retirer un owner (même soi-même) : owner uniquement.
        if !matches!(membership.role, OrgMemberRole::Owner) {
            return Err(forbidden("owner requis pour retirer un owner"));
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
                "l'organisation doit garder au moins un owner (transférez le rôle ou supprimez l'org)",
            ));
        }
    }

    organization_members::Entity::delete_by_id(target.id)
        .exec(&ctx.db)
        .await
        .map_err(|_| Error::InternalServerError)?;
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
