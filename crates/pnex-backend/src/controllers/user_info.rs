//! `GET /api/v1/user-info` — parité fonctionnelle du contrat Django, adaptée
//! multi-tenant : le profil reste par utilisateur, l'abonnement est porté par
//! les organisations (D11). `device_count` agrège sur les orgs dont l'utilisateur
//! est membre (les devices sont scoping org depuis la Phase 2).
//!
//! `PATCH /api/v1/profile` — préférences du profil (langue, timezone, format
//! de date, thème) : consommé par la page Profil et le switcher de langue du
//! front.

use axum::extract::State;
use loco_rs::prelude::*;
use sea_orm::{ColumnTrait, EntityTrait, QueryFilter, QueryOrder};
use serde::Deserialize;

use crate::auth::AuthUser;
use crate::models::_entities::{
    device_registries, device_types, organization_members, organizations, predefined_devices,
    sea_orm_active_enums::UiTheme, subscription_tiers, user_profiles,
};

/// Bloc `profile` tel qu'exposé par `GET /user-info` et `PATCH /profile`
/// (forme unique).
fn profile_json(profile: &user_profiles::Model) -> serde_json::Value {
    serde_json::json!({
        "language": profile.language,
        "timezone": profile.timezone,
        "date_format": profile.date_format,
        "theme": profile.theme,
        "preferences": profile.preferences,
        "grafana_url": profile.grafana_url,
        "llm_endpoint_openapi_compatible": profile.llm_endpoint_openapi_compatible,
        "llm_token": profile.llm_token,
        "llm_model": profile.llm_model,
    })
}

pub async fn user_info(State(ctx): State<AppContext>, auth: AuthUser) -> Result<Response> {
    let db = &ctx.db;
    let user = auth.user;

    let profile = user_profiles::Entity::find()
        .filter(user_profiles::Column::UserId.eq(user.id))
        .one(db)
        .await
        .map_err(|_| Error::InternalServerError)?;

    // Orgs dont l'utilisateur est membre, avec tier et rôle. Ordre déterministe
    // par id d'org croissant (l'org personnelle JIT d'abord) — sans cela
    // l'ordre est arbitraire côté Postgres et le fallback d'org du front peut
    // atterrir sur une org viewer (observation O1).
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
            "role": crate::controllers::orgs::role_str(membership.role),
            "subscription_tier": tier,
        }));
    }

    // Comptage devices sur les orgs du user (via predefined → device_type).
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
        "username": auth.claims.preferred_username,
        "email": user.email,
        "full_name": user.full_name,
        "profile": profile.as_ref().map(profile_json),
        "orgs": orgs,
        "device_count": {
            "total": total,
            "active": active,
            "by_type": by_type,
        },
    });
    format::json(body)
}

/// Corps accepté par `PATCH /api/v1/profile` (DTO partagé `pnex-core`).
#[derive(Deserialize, Default)]
pub struct ProfilePatch {
    pub language: Option<String>,
    pub timezone: Option<String>,
    pub date_format: Option<String>,
    pub theme: Option<String>,
}

/// Langue acceptée (formes courtes UI normalisées en stockage) : en, fr.
fn normalize_language(input: &str) -> Option<String> {
    match input.to_ascii_lowercase().as_str() {
        "en" | "en-us" => Some("en".into()),
        "fr" | "fr-fr" => Some("fr".into()),
        _ => None,
    }
}

/// Thème accepté (minuscules, parité string_value SeaORM).
fn parse_theme(input: &str) -> Option<UiTheme> {
    match input {
        "light" => Some(UiTheme::Light),
        "dark" => Some(UiTheme::Dark),
        "auto" => Some(UiTheme::Auto),
        _ => None,
    }
}

/// `PATCH /api/v1/profile` — met à jour les préférences du profil de
/// l'utilisateur authentifié. Champs fournis uniquement, valeurs invalides →
/// 400 (message français), renvoie le bloc `profile` à jour.
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
        return Err(Error::BadRequest(
            "au moins un champ est requis (language, timezone, date_format, theme)".into(),
        ));
    }

    let language = match patch.language.as_deref() {
        None => None,
        Some(raw) => {
            let trimmed = raw.trim();
            if trimmed.is_empty() {
                return Err(Error::BadRequest("language ne peut pas être vide".into()));
            }
            Some(
                normalize_language(trimmed)
                    .ok_or_else(|| Error::BadRequest("language non supportée (en, fr)".into()))?,
            )
        }
    };
    let timezone = match patch.timezone.as_deref() {
        None => None,
        Some(raw) => {
            let trimmed = raw.trim();
            // Bornes = colonnes (timezone 50, date_format 20).
            if trimmed.is_empty() || trimmed.len() > 50 {
                return Err(Error::BadRequest("timezone invalide".into()));
            }
            Some(trimmed.to_string())
        }
    };
    let date_format = match patch.date_format.as_deref() {
        None => None,
        Some(raw) => {
            let trimmed = raw.trim();
            if trimmed.is_empty() || trimmed.len() > 20 {
                return Err(Error::BadRequest("date_format invalide".into()));
            }
            Some(trimmed.to_string())
        }
    };
    let theme =
        match patch.theme.as_deref() {
            None => None,
            Some(raw) => {
                let trimmed = raw.trim();
                if trimmed.is_empty() {
                    return Err(Error::BadRequest("theme ne peut pas être vide".into()));
                }
                Some(parse_theme(trimmed).ok_or_else(|| {
                    Error::BadRequest("theme invalide (light, dark, auto)".into())
                })?)
            }
        };

    // Le profil est créé par le JIT provisioning ; par robustesse on le crée
    // avec les défauts s'il manque (ordre d'appels en test, reprise de données).
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
