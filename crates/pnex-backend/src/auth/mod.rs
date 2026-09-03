//! Authentification Rauthy (IdP OIDC) : extracteurs Axum.
//!
//! - [`AuthUser`] : `Authorization: Bearer <jwt>` validé par JWKS (RS256,
//!   `iss`/`aud`/`exp`), puis JIT provisioning (`users` + profil + org
//!   personnelle). Refus par défaut : sans token valide → 401.
//! - [`OrgContext`] : [`AuthUser`] + sélection d'org via l'en-tête
//!   `X-Org-Id`, vérifiée contre le membership → 403 si non membre.
//!   C'est le point d'ancrage du scoping multi-tenant (remplace le filtrage
//!   par-viewset du Django POC, rapport Phase 0 §3).

pub mod claims;
pub mod jwks;
pub mod provisioning;
pub mod settings;

use axum::extract::FromRequestParts;
use axum::http::header::AUTHORIZATION;
use axum::http::request::Parts;
use loco_rs::prelude::*;
use sea_orm::ExprTrait;

use crate::models::_entities::{
    organization_members, organizations, sea_orm_active_enums::OrgMemberRole, users,
};

use claims::Claims;

/// Challenge `WWW-Authenticate` (parité Django `authenticate_header`).
const WWW_AUTHENTICATE: &str = "Bearer realm=\"api\"";

pub struct AuthUser {
    pub claims: Claims,
    pub user: users::Model,
}

fn bearer_token(parts: &Parts) -> Option<String> {
    let value = parts.headers.get(AUTHORIZATION)?.to_str().ok()?;
    let token = value
        .strip_prefix("Bearer ")
        .or_else(|| value.strip_prefix("bearer "))?;
    if token.is_empty() {
        None
    } else {
        Some(token.trim().to_string())
    }
}

impl FromRequestParts<AppContext> for AuthUser {
    type Rejection = loco_rs::Error;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppContext,
    ) -> Result<Self, Self::Rejection> {
        let token = bearer_token(parts)
            .ok_or_else(|| loco_rs::Error::Unauthorized(WWW_AUTHENTICATE.to_string()))?;

        let settings = settings::RauthySettings::from_config(&state.config)?;
        let verifier = jwks::verifier_for(&settings).await;
        let claims = verifier.verify(&token).await.map_err(|err| {
            tracing::warn!(%err, "rejet JWT");
            loco_rs::Error::Unauthorized(WWW_AUTHENTICATE.to_string())
        })?;

        let user = provisioning::get_or_create_user(&state.db, &claims)
            .await
            .map_err(|err| {
                tracing::error!(%err, "JIT provisioning échoué");
                match err {
                    provisioning::ProvisionError::MissingEmail => {
                        loco_rs::Error::Unauthorized(err.to_string())
                    }
                    _ => loco_rs::Error::InternalServerError,
                }
            })?;

        Ok(AuthUser { claims, user })
    }
}

/// Utilisateur authentifié + org sélectionnée (`X-Org-Id`) + rôle effectif.
pub struct OrgContext {
    pub auth: AuthUser,
    pub org: organizations::Model,
    pub role: OrgMemberRole,
}

impl OrgContext {
    /// Accès en écriture : owner ou admin.
    pub fn can_write(&self) -> bool {
        matches!(self.role, OrgMemberRole::Owner | OrgMemberRole::Admin)
    }

    /// Administration de l'org : owner uniquement.
    pub fn is_owner(&self) -> bool {
        matches!(self.role, OrgMemberRole::Owner)
    }
}

impl FromRequestParts<AppContext> for OrgContext {
    type Rejection = loco_rs::Error;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppContext,
    ) -> Result<Self, Self::Rejection> {
        let auth = AuthUser::from_request_parts(parts, state).await?;

        let org_id: i64 = parts
            .headers
            .get("X-Org-Id")
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.parse().ok())
            .ok_or_else(|| {
                loco_rs::Error::BadRequest(
                    "en-tête X-Org-Id requis (id de l'organisation sélectionnée)".into(),
                )
            })?;

        let membership = organization_members::Entity::find()
            .filter(
                organization_members::Column::UserId
                    .eq(auth.user.id)
                    .and(organization_members::Column::OrgId.eq(org_id)),
            )
            .one(&state.db)
            .await
            .map_err(|_| loco_rs::Error::InternalServerError)?;

        let Some(membership) = membership else {
            // 403 et non 404 : l'utilisateur a fourni un id, on lui dit
            // qu'il n'en est pas membre (isolation par cloisonnement
            // fonctionnel, pas par obscurcissement).
            return Err(loco_rs::Error::CustomError(
                axum::http::StatusCode::FORBIDDEN,
                loco_rs::controller::ErrorDetail::new(
                    "forbidden",
                    "vous n'êtes pas membre de cette organisation".to_string(),
                ),
            ));
        };

        let org = organizations::Entity::find_by_id(org_id)
            .one(&state.db)
            .await
            .map_err(|_| loco_rs::Error::InternalServerError)?
            .ok_or(loco_rs::Error::NotFound)?;

        Ok(OrgContext {
            role: membership.role,
            org,
            auth,
        })
    }
}
