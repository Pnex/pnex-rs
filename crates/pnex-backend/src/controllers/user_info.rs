//! `GET /api/v1/user-info` — parité fonctionnelle du contrat Django, adaptée
//! multi-tenant : le profil reste par utilisateur, l'abonnement est porté par
//! les organisations (D11). `device_count` agrège sur les orgs dont l'utilisateur
//! est membre (les devices sont scoping org depuis la Phase 2).

use axum::extract::State;
use loco_rs::prelude::*;
use sea_orm::{ColumnTrait, EntityTrait, QueryFilter};

use crate::auth::AuthUser;
use crate::models::_entities::{
    device_registries, device_types, organization_members, organizations, predefined_devices,
    subscription_tiers, user_profiles,
};

pub async fn user_info(
    State(ctx): State<AppContext>,
    auth: AuthUser,
) -> Result<Response> {
    let db = &ctx.db;
    let user = auth.user;

    let profile = user_profiles::Entity::find()
        .filter(user_profiles::Column::UserId.eq(user.id))
        .one(db)
        .await
        .map_err(|_| Error::InternalServerError)?;

    // Orgs dont l'utilisateur est membre, avec tier et rôle.
    let memberships = organization_members::Entity::find()
        .filter(organization_members::Column::UserId.eq(user.id))
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
            .map(|t| serde_json::json!({
                "name": t.name,
                "max_sensor_devices": t.max_sensor_devices,
                "max_actuator_devices": t.max_actuator_devices,
                "max_mixed_devices": t.max_mixed_devices,
            }));
        orgs.push(serde_json::json!({
            "id": org.id,
            "name": org.name,
            "role": crate::controllers::orgs::role_str(membership.role),
            "subscription_tier": tier,
        }));
    }

    // Comptage devices sur les orgs du user (via predefined → device_type).
    let mut by_type: serde_json::Map<String, serde_json::Value> =
        serde_json::Map::new();
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
        "profile": profile.map(|p| serde_json::json!({
            "language": p.language,
            "timezone": p.timezone,
            "date_format": p.date_format,
            "theme": p.theme,
            "preferences": p.preferences,
            "grafana_url": p.grafana_url,
            "llm_endpoint_openapi_compatible": p.llm_endpoint_openapi_compatible,
            "llm_token": p.llm_token,
            "llm_model": p.llm_model,
        })),
        "orgs": orgs,
        "device_count": {
            "total": total,
            "active": active,
            "by_type": by_type,
        },
    });
    format::json(body)
}

pub fn routes() -> Routes {
    Routes::new()
        .prefix("/api/v1")
        .add("/user-info", get(user_info))
}
