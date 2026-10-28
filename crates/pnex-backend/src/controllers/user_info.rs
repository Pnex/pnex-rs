//! `GET /api/v1/user-info` — multi-tenant model: the profile stays per user, the
//! subscription is carried by the organizations (D11). `device_count`
//! aggregates over the orgs the user is a member of (devices are org-scoped
//! since Phase 2).
//!
//! `PATCH /api/v1/profile`: profile preferences (language, timezone, date
//! format, theme), used by the Profile page and the language switcher.

use axum::extract::State;
use loco_rs::prelude::*;
use sea_orm::{ColumnTrait, EntityTrait, QueryFilter, QueryOrder};
use serde::Deserialize;

use crate::auth::AuthUser;
use crate::controllers::field_error;
use crate::models::_entities::{
    device_registries, device_types, organization_members, organizations, predefined_devices,
    sea_orm_active_enums::UiTheme, subscription_tiers, user_profiles,
};
use pnex_core::err_codes::{FIELD_INVALID, FIELD_REQUIRED};

/// `profile` block as exposed by `GET /user-info` and `PATCH /profile`
/// (single shape).
/// Wire form of the stored theme (lowercase `pnex_core::UiTheme`).
fn api_theme(theme: UiTheme) -> pnex_core::UiTheme {
    match theme {
        UiTheme::Light => pnex_core::UiTheme::Light,
        UiTheme::Dark => pnex_core::UiTheme::Dark,
        UiTheme::Auto => pnex_core::UiTheme::Auto,
    }
}

fn profile_json(profile: &user_profiles::Model) -> serde_json::Value {
    serde_json::json!({
        "language": profile.language,
        "timezone": profile.timezone,
        "date_format": profile.date_format,
        "theme": api_theme(profile.theme),
        "preferences": profile.preferences,
        "grafana_url": profile.grafana_url,
    })
}

#[utoipa::path(
    get,
    path = "/api/v1/user-info",
    tag = "user-info",
    operation_id = "user_info_get",
    summary = "Current user, profile and org memberships",
    description = "Identity of the caller, profile preferences (null until provisioned), every org the user belongs to (ordered by org id, with role and subscription tier), platform-admin flag, deployment mode (`self_hosted` | `saas`) and a device count aggregated over the user's orgs. Not org-scoped.",
    responses((status = 200, body = pnex_core::UserInfo)),
    security(("bearer" = []))
)]
pub async fn user_info(State(ctx): State<AppContext>, auth: AuthUser) -> Result<Response> {
    let db = &ctx.db;
    let user = auth.user;

    let profile = user_profiles::Entity::find()
        .filter(user_profiles::Column::UserId.eq(user.id))
        .one(db)
        .await
        .map_err(|_| Error::InternalServerError)?;

    // Orgs the user is a member of, with tier and role. Deterministic order
    // by ascending org id (the JIT personal org first) — without it
    // the order is arbitrary in Postgres and the front's org fallback can
    // land on a viewer org (observation O1).
    let memberships = organization_members::Entity::find()
        .filter(organization_members::Column::UserId.eq(user.id))
        .order_by_asc(organization_members::Column::OrgId)
        .find_also_related(organizations::Entity)
        .all(db)
        .await
        .map_err(|_| Error::InternalServerError)?;

    let tiers: std::collections::HashMap<i64, subscription_tiers::Model> =
        subscription_tiers::Entity::find()
            .all(db)
            .await
            .map_err(|_| Error::InternalServerError)?
            .into_iter()
            .map(|t| (t.id, t))
            .collect();

    let mut orgs = Vec::with_capacity(memberships.len());
    let mut org_ids = Vec::with_capacity(memberships.len());
    for (membership, org) in memberships {
        let Some(org) = org else { continue };
        org_ids.push(org.id);
        let tier = org
            .subscription_tier_id
            .and_then(|id| tiers.get(&id))
            .map(|t| {
                serde_json::json!({
                    "name": t.name,
                    "max_sensor_devices": t.max_sensor_devices,
                    "max_actuator_devices": t.max_actuator_devices,
                    "max_mixed_devices": t.max_mixed_devices,
                })
            });
        orgs.push(serde_json::json!({
            "id": org.id,
            "name": org.name,
            "role": crate::controllers::orgs::api_role(membership.role),
            "subscription_tier": tier,
        }));
    }

    // Device count over the user's orgs (via predefined → device_type).
    let mut by_type: serde_json::Map<String, serde_json::Value> = serde_json::Map::new();
    let mut total = 0usize;
    let mut active = 0usize;
    if !org_ids.is_empty() {
        let devices = device_registries::Entity::find()
            .filter(device_registries::Column::OrgId.is_in(org_ids))
            .find_also_related(predefined_devices::Entity)
            .all(db)
            .await
            .map_err(|_| Error::InternalServerError)?;

        let types: std::collections::HashMap<i64, String> = device_types::Entity::find()
            .all(db)
            .await
            .map_err(|_| Error::InternalServerError)?
            .into_iter()
            .map(|t| (t.id, t.name))
            .collect();

        for (device, predefined) in devices {
            total += 1;
            if device.active {
                active += 1;
            }
            let type_name = predefined
                .as_ref()
                .and_then(|p| types.get(&p.device_type_id))
                .cloned()
                .unwrap_or_else(|| "unknown".into());
            let entry = by_type.entry(type_name).or_insert(0u64.into());
            if let serde_json::Value::Number(n) = entry {
                if let Some(v) = n.as_u64() {
                    *entry = (v + 1).into();
                }
            }
        }
    }

    let body = serde_json::json!({
        "id": user.id,
        // Lean Rauthy access tokens: no preferred_username (it lives in
        // the id_token) → fallback to the provisioned user's email. Null is
        // forbidden here: the UserInfo contract expects a string (D19).
        "username": auth.claims.preferred_username.clone().unwrap_or_else(|| user.email.clone()),
        "email": user.email,
        "full_name": user.full_name,
        "profile": profile.as_ref().map(profile_json),
        "orgs": orgs,
        "platform_admin": user.platform_admin,
        "deployment_mode": pnex_core::DeploymentMode::from(
            crate::services::retention::DeploymentMode::from_env(),
        ),
        "device_count": {
            "total": total,
            "active": active,
            "by_type": by_type,
        },
    });
    format::json(body)
}

/// Body of `PATCH /api/v1/profile` (shared DTO in `pnex-core`).
#[derive(Deserialize, Default)]
pub struct ProfilePatch {
    pub language: Option<String>,
    pub timezone: Option<String>,
    pub date_format: Option<String>,
    pub theme: Option<String>,
}

/// Accepted language (UI short forms, normalized for storage): en, fr.
fn normalize_language(input: &str) -> Option<String> {
    match input.to_ascii_lowercase().as_str() {
        "en" | "en-us" => Some("en".into()),
        "fr" | "fr-fr" => Some("fr".into()),
        _ => None,
    }
}

/// Accepted theme (lowercase, same as the SeaORM string_value).
fn parse_theme(input: &str) -> Option<UiTheme> {
    match input {
        "light" => Some(UiTheme::Light),
        "dark" => Some(UiTheme::Dark),
        "auto" => Some(UiTheme::Auto),
        _ => None,
    }
}

/// `PATCH /api/v1/profile`: updates the preferences of the caller. Only the
/// provided fields change; an invalid value answers a `field-invalid` 400.
/// Returns the updated `profile` block.
#[utoipa::path(
    patch,
    path = "/api/v1/profile",
    tag = "profile",
    operation_id = "profile_update",
    summary = "Update the caller's profile preferences",
    description = "Partial update of the authenticated user's preferences: `language` (en | fr, en-US/fr-FR accepted), `timezone` (max 50 chars), `date_format` (max 20 chars), `theme` (light | dark | auto). At least one field is required. Creates the profile with defaults if missing. Returns the updated profile block.",
    request_body = pnex_core::ProfilePatch,
    responses(
        (status = 200, body = pnex_core::UserProfile),
        (status = 400, description = "`profile-patch-empty` (no field), or `field-invalid` with `errors.fields` (`required`, `invalid`)", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = []))
)]
async fn update_profile(
    State(ctx): State<AppContext>,
    auth: AuthUser,
    Json(patch): Json<ProfilePatch>,
) -> Result<Response> {
    if patch.language.is_none()
        && patch.timezone.is_none()
        && patch.date_format.is_none()
        && patch.theme.is_none()
    {
        return Err(crate::controllers::coded_error(
            axum::http::StatusCode::BAD_REQUEST,
            pnex_core::err_codes::PROFILE_PATCH_EMPTY,
            "at least one field is required (language, timezone, date_format, theme)",
            None,
        ));
    }

    let language = match patch.language.as_deref() {
        None => None,
        Some(raw) => {
            let trimmed = raw.trim();
            if trimmed.is_empty() {
                return Ok(field_error("language", FIELD_REQUIRED));
            }
            let Some(language) = normalize_language(trimmed) else {
                return Ok(field_error("language", FIELD_INVALID));
            };
            Some(language)
        }
    };
    let timezone = match patch.timezone.as_deref() {
        None => None,
        Some(raw) => {
            let trimmed = raw.trim();
            // Bounds = columns (timezone 50, date_format 20).
            if trimmed.is_empty() || trimmed.len() > 50 {
                return Ok(field_error("timezone", FIELD_INVALID));
            }
            Some(trimmed.to_string())
        }
    };
    let date_format = match patch.date_format.as_deref() {
        None => None,
        Some(raw) => {
            let trimmed = raw.trim();
            if trimmed.is_empty() || trimmed.len() > 20 {
                return Ok(field_error("date_format", FIELD_INVALID));
            }
            Some(trimmed.to_string())
        }
    };
    let theme = match patch.theme.as_deref() {
        None => None,
        Some(raw) => {
            let trimmed = raw.trim();
            if trimmed.is_empty() {
                return Ok(field_error("theme", FIELD_REQUIRED));
            }
            let Some(theme) = parse_theme(trimmed) else {
                return Ok(field_error("theme", FIELD_INVALID));
            };
            Some(theme)
        }
    };

    // The profile is created by JIT provisioning; for robustness it is created
    // with defaults if missing (call order in tests, data recovery).
    let existing = user_profiles::Entity::find()
        .filter(user_profiles::Column::UserId.eq(auth.user.id))
        .one(&ctx.db)
        .await
        .map_err(|_| Error::InternalServerError)?;
    let profile = match existing {
        Some(model) => model,
        None => user_profiles::ActiveModel {
            user_id: sea_orm::Set(auth.user.id),
            ..Default::default()
        }
        .insert(&ctx.db)
        .await
        .map_err(|_| Error::InternalServerError)?,
    };

    let mut active: user_profiles::ActiveModel = profile.into();
    if let Some(language) = language {
        active.language = sea_orm::Set(language);
    }
    if let Some(timezone) = timezone {
        active.timezone = sea_orm::Set(timezone);
    }
    if let Some(date_format) = date_format {
        active.date_format = sea_orm::Set(Some(date_format));
    }
    if let Some(theme) = theme {
        active.theme = sea_orm::Set(theme);
    }
    let updated = active
        .update(&ctx.db)
        .await
        .map_err(|_| Error::InternalServerError)?;
    format::json(profile_json(&updated))
}

pub fn routes() -> Routes {
    Routes::new()
        .prefix("/api/v1")
        .add("/user-info", get(user_info))
        .add("/profile", patch(update_profile))
}

/// OpenAPI paths of this controller (merged by `crate::openapi::spec`).
pub fn openapi() -> utoipa::openapi::OpenApi {
    #[derive(utoipa::OpenApi)]
    #[openapi(paths(user_info, update_profile))]
    struct Doc;
    <Doc as utoipa::OpenApi>::openapi()
}
