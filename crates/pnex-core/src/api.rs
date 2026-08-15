//! DTO de l'API `/api/v1` — source unique partagée backend ↔ frontend.
//!
//! Ces types reflètent les payloads des contrôleurs Phase 3 (proxy OAuth2,
//! user-info, orgs) et du `PATCH /api/v1/profile`. Les dates arrivent en
//! chaînes RFC 3339 (sérialisation SeaORM) — pas de chrono ici, le core
//! reste dépendance-free (serde uniquement).
//!
//! Rôles : strings minuscules (« owner », « admin », « viewer ») — convention
//! API (les enums SeaORM générés sérialisent en Capitalized, on mappe côté
//! backend via `role_str`/`RoleParam`).

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

/// Réponse du proxy OAuth2 (`/oauth2/token`, `/oauth2/refresh`) — relay de
/// Keycloak (champs du grant flow standard).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TokenResponse {
    pub access_token: String,
    pub refresh_token: String,
    pub expires_in: i64,
    pub token_type: String,
}

/// `GET /api/v1/user-info` — identité + profil + orgs + comptage devices.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserInfo {
    pub id: i64,
    pub username: String,
    #[serde(default)]
    pub email: Option<String>,
    #[serde(default)]
    pub full_name: Option<String>,
    #[serde(default)]
    pub profile: Option<UserProfile>,
    #[serde(default)]
    pub orgs: Vec<OrgMembership>,
    pub device_count: DeviceCount,
}

/// Bloc `profile` de `user-info` (préférences utilisateur).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserProfile {
    pub language: String,
    pub timezone: String,
    #[serde(default)]
    pub date_format: Option<String>,
    /// « Light » / « Dark » / « Auto » côté storage (enum SeaORM Capitalized) ;
    /// minuscules en entrée du PATCH.
    pub theme: String,
    #[serde(default)]
    pub preferences: Option<serde_json::Value>,
    #[serde(default)]
    pub grafana_url: Option<String>,
    #[serde(default)]
    pub llm_endpoint_openapi_compatible: Option<String>,
    #[serde(default)]
    pub llm_token: Option<String>,
    #[serde(default)]
    pub llm_model: Option<String>,
}

/// Appartenance org d'un utilisateur, telle que renvoyée par `user-info`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OrgMembership {
    pub id: i64,
    pub name: String,
    /// « owner » | « admin » | « viewer ».
    pub role: String,
    #[serde(default)]
    pub subscription_tier: Option<TierInfo>,
}

/// Tier d'abonnement d'une org (quotas).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TierInfo {
    pub name: String,
    pub max_sensor_devices: i64,
    pub max_actuator_devices: i64,
    pub max_mixed_devices: i64,
}

/// Comptage devices agrégé sur les orgs de l'utilisateur.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeviceCount {
    pub total: u64,
    pub active: u64,
    #[serde(default)]
    pub by_type: HashMap<String, u64>,
}

/// `GET /api/v1/orgs` — orgs dont je suis membre.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OrgSummary {
    pub id: i64,
    pub name: String,
    /// « owner » | « admin » | « viewer ».
    pub role: String,
    #[serde(default)]
    pub subscription_tier: Option<String>,
    #[serde(default)]
    pub created_at: Option<String>,
}

/// `GET /api/v1/orgs/{id}` — détail avec membres.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OrgDetail {
    pub id: i64,
    pub name: String,
    #[serde(default)]
    pub subscription_tier_id: Option<i64>,
    /// Rôle de l'utilisateur courant dans cette org.
    pub role: String,
    pub members: Vec<OrgMember>,
}

/// Membre d'une organisation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OrgMember {
    pub user_id: i64,
    #[serde(default)]
    pub email: Option<String>,
    #[serde(default)]
    pub full_name: Option<String>,
    /// « owner » | « admin » | « viewer ».
    pub role: String,
    #[serde(default)]
    pub created_at: Option<String>,
}

/// Corps du `PATCH /api/v1/profile` — champs optionnels, formulé par le front.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ProfilePatch {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub language: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timezone: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub date_format: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub theme: Option<String>,
}

/// Corps du `POST /api/v1/orgs`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateOrg {
    pub name: String,
}

/// Corps du `PATCH /api/v1/orgs/{id}`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpdateOrg {
    pub name: String,
}

/// Corps du `POST /api/v1/orgs/{id}/members`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AddMember {
    pub email: String,
    /// « owner » | « admin » | « viewer » (défaut « viewer » côté API).
    pub role: Option<String>,
}

/// Corps du `PATCH /api/v1/orgs/{id}/members/{user_id}`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpdateMember {
    /// « owner » | « admin » | « viewer ».
    pub role: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn user_info_minimal_deserialize() {
        // Forme minimale renvoyée par le backend (profile/orgs absents ou vides).
        let json = r#"{
            "id": 1,
            "username": "alice",
            "email": null,
            "full_name": null,
            "profile": null,
            "orgs": [],
            "device_count": { "total": 0, "active": 0, "by_type": {} }
        }"#;
        let info: UserInfo = serde_json::from_str(json).unwrap();
        assert_eq!(info.username, "alice");
        assert!(info.orgs.is_empty());
    }

    #[test]
    fn orgs_shapes_deserialize() {
        let list = r#"[{
            "id": 3, "name": "Atelier Co", "role": "owner",
            "subscription_tier": "Free", "created_at": "2026-08-15T10:00:00+00:00"
        }]"#;
        let orgs: Vec<OrgSummary> = serde_json::from_str(list).unwrap();
        assert_eq!(orgs[0].subscription_tier.as_deref(), Some("Free"));

        let detail = r#"{
            "id": 3, "name": "Atelier Co", "subscription_tier_id": 1, "role": "owner",
            "members": [{ "user_id": 2, "email": "bob@example.com", "full_name": null,
                          "role": "viewer", "created_at": null }]
        }"#;
        let detail: OrgDetail = serde_json::from_str(detail).unwrap();
        assert_eq!(detail.members[0].role, "viewer");
    }

    #[test]
    fn profile_patch_skip_none() {
        let patch = ProfilePatch {
            language: Some("fr-FR".into()),
            ..Default::default()
        };
        let json = serde_json::to_string(&patch).unwrap();
        assert_eq!(json, r#"{"language":"fr-FR"}"#);
    }
}
