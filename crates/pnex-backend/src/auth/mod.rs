//! Authentification Rauthy (IdP OIDC) : extracteurs Axum.
//!
//! - [`AuthUser`] : `Authorization: Bearer <jwt>` validé par JWKS (RS256,
//!   `iss`/`aud`/`exp`), puis JIT provisioning (`users` + profil + org
//!   personnelle). Refus par défaut : sans token valide → 401.
//! - [`OrgContext`] : [`AuthUser`] + sélection d'org via l'en-tête
//!   `X-Org-Id`, vérifiée contre le membership → 403 si non membre.
//!   This is the anchor of multi-tenant scoping (Phase 0 report §3).
//! - [`PlatformAdmin`]: [`AuthUser`] with `users.platform_admin = true`
//!   (infra status, global retention) → 403 `platform-admin-required`.

pub mod claims;
pub mod internal_guard;
pub mod jwks;
pub mod provisioning;
pub mod service_token;
pub mod settings;
pub mod user_cache;

use axum::extract::FromRequestParts;
use axum::http::header::AUTHORIZATION;
use axum::http::request::Parts;
use loco_rs::prelude::*;
use sea_orm::ExprTrait;

use pnex_api_contract::iam::Access;

use crate::models::_entities::{
    organization_members, organizations, sea_orm_active_enums::OrgMemberRole, users,
};

use claims::Claims;

/// `WWW-Authenticate` challenge.
const WWW_AUTHENTICATE: &str = "Bearer realm=\"api\"";

pub struct AuthUser {
    pub claims: Claims,
    pub user: users::Model,
    /// Set when the caller is a service account (access key, iam.md §7).
    pub service: Option<crate::services::iam::service::ServicePrincipal>,
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

/// A human signed in through the IdP: an access key is refused here, it
/// only calls org operations (`OrgContext`, iam.md §7).
impl FromRequestParts<AppContext> for AuthUser {
    type Rejection = loco_rs::Error;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppContext,
    ) -> Result<Self, Self::Rejection> {
        let auth = authenticate(parts, state).await?;
        if auth.service.is_some() {
            return Err(access_key_not_allowed());
        }
        Ok(auth)
    }
}

fn access_key_not_allowed() -> loco_rs::Error {
    crate::controllers::coded_error(
        axum::http::StatusCode::FORBIDDEN,
        pnex_core::err_codes::ACCESS_KEY_NOT_ALLOWED,
        "an access key only calls the org operations its roles allow, read-only",
        None,
    )
}

/// Claims of a service account, so code reading `claims` stays uniform.
fn service_claims(
    user: &users::Model,
    sp: &crate::services::iam::service::ServicePrincipal,
) -> Claims {
    Claims {
        sub: format!("service:{}", sp.id),
        preferred_username: user.full_name.clone(),
        email: Some(user.email.clone()),
        email_verified: Some(false),
        given_name: None,
        family_name: None,
        iss: "pnex:access-key".to_string(),
        exp: sp.expires_at.timestamp(),
        aud: None,
    }
}

/// Bearer of the request: an IdP access token, or an access key
/// (`pnex_sk_…`) of a service account.
async fn authenticate(parts: &Parts, state: &AppContext) -> Result<AuthUser, loco_rs::Error> {
    let token = bearer_token(parts)
        .ok_or_else(|| loco_rs::Error::Unauthorized(WWW_AUTHENTICATE.to_string()))?;

    if token.starts_with(crate::services::iam::service::KEY_PREFIX) {
        let found = crate::services::iam::service::authenticate(&state.db, &token)
            .await
            .map_err(|e| {
                tracing::error!(error = %e, "access key lookup failed");
                loco_rs::Error::InternalServerError
            })?;
        let Some((user, sp)) = found else {
            return Err(loco_rs::Error::Unauthorized(WWW_AUTHENTICATE.to_string()));
        };
        return Ok(AuthUser {
            claims: service_claims(&user, &sp),
            user,
            service: Some(sp),
        });
    }

    let settings = settings::RauthySettings::from_config(&state.config)?;
    let verifier = jwks::verifier_for(&settings).await;
    let claims = verifier.verify(&token).await.map_err(|err| {
        tracing::warn!(%err, "JWT rejected");
        loco_rs::Error::Unauthorized(WWW_AUTHENTICATE.to_string())
    })?;

    // Short-TTL per-pod cache (see `user_cache`): skips the lookup and
    // the drift check while the token profile is unchanged.
    let email = claims.email.clone().unwrap_or_default();
    let display_name = claims.display_name();
    let ttl = user_cache::ttl(&state.config);
    if let Some(user) = user_cache::get(&claims.sub, &email, &display_name, ttl) {
        return Ok(AuthUser {
            claims,
            user,
            service: None,
        });
    }

    let user = provisioning::get_or_create_user(&state.db, &claims)
        .await
        .map_err(|err| {
            tracing::error!(%err, "JIT provisioning failed");
            match err {
                provisioning::ProvisionError::MissingEmail
                | provisioning::ProvisionError::UnverifiedEmailRelink => {
                    loco_rs::Error::Unauthorized(err.to_string())
                }
                _ => loco_rs::Error::InternalServerError,
            }
        })?;
    if !ttl.is_zero() {
        user_cache::put(&claims.sub, &email, &display_name, user.clone());
    }

    Ok(AuthUser {
        claims,
        user,
        service: None,
    })
}

/// Utilisateur authentifié + org sélectionnée (`X-Org-Id`) + rôle effectif.
pub struct OrgContext {
    pub auth: AuthUser,
    pub org: organizations::Model,
    /// Managed role of the membership (D117).
    pub role: OrgMemberRole,
    /// Membership role plus assigned roles (iam.md D286): what the engine
    /// decides with.
    pub grants: crate::services::iam::Grants,
}

impl OrgContext {
    /// Does the caller hold `permission` (`service:verb`) in this org?
    /// For checks inside a handler (a field gate, a redacted field); the
    /// operation's own permission is already enforced by the extractor.
    pub fn can(&self, permission: &str) -> bool {
        self.grants.can(permission)
    }
}

impl FromRequestParts<AppContext> for OrgContext {
    type Rejection = loco_rs::Error;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppContext,
    ) -> Result<Self, Self::Rejection> {
        let auth = authenticate(parts, state).await?;
        let header_org: Option<i64> = parts
            .headers
            .get("X-Org-Id")
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.parse().ok());
        let not_member = || {
            // 403 et non 404 : l'utilisateur a fourni un id, on lui dit
            // qu'il n'en est pas membre (isolation par cloisonnement
            // fonctionnel, pas par obscurcissement).
            crate::controllers::coded_error(
                axum::http::StatusCode::FORBIDDEN,
                pnex_core::err_codes::ORG_NOT_MEMBER,
                "you are not a member of this organisation",
                None,
            )
        };

        let (org, role, grants) = if let Some(sp) = &auth.service {
            // A service account belongs to one org: `X-Org-Id` may only
            // name it. It holds its assignments, nothing else (iam.md §7).
            if header_org.is_some_and(|o| o != sp.org_id) {
                return Err(not_member());
            }
            let org = organizations::Entity::find_by_id(sp.org_id)
                .one(&state.db)
                .await
                .map_err(|_| loco_rs::Error::InternalServerError)?
                .ok_or_else(not_member)?;
            let grants = crate::services::iam::store::grants(&state.db, org.id, auth.user.id, None)
                .await
                .map_err(|e| {
                    tracing::error!(error = %e, "iam grants query failed");
                    loco_rs::Error::InternalServerError
                })?;
            (org, OrgMemberRole::Viewer, grants)
        } else {
            let org_id = header_org.ok_or_else(|| {
                crate::controllers::coded_error(
                    axum::http::StatusCode::BAD_REQUEST,
                    pnex_core::err_codes::ORG_HEADER_REQUIRED,
                    "the X-Org-Id header (id of the selected organisation) is required",
                    None,
                )
            })?;

            // Membership + organization in one joined query, never cached:
            // role changes and removals apply immediately on every pod.
            let row = organization_members::Entity::find()
                .filter(
                    organization_members::Column::UserId
                        .eq(auth.user.id)
                        .and(organization_members::Column::OrgId.eq(org_id)),
                )
                .find_also_related(organizations::Entity)
                .one(&state.db)
                .await
                .map_err(|_| loco_rs::Error::InternalServerError)?;

            let Some((membership, org)) = row else {
                return Err(not_member());
            };

            let org = org.ok_or(loco_rs::Error::NotFound)?;

            // Never cached either: an assignment change applies on the next call.
            let grants = crate::services::iam::store::grants(
                &state.db,
                org.id,
                auth.user.id,
                Some(&membership.role),
            )
            .await
            .map_err(|e| {
                tracing::error!(error = %e, "iam grants query failed");
                loco_rs::Error::InternalServerError
            })?;
            (org, membership.role, grants)
        };

        // The engine decides first (iam.md D291): the permission the action
        // registry gives this operation.
        let matched = parts.extensions.get::<axum::extract::MatchedPath>();
        let mut refused = None;
        let decision = match crate::services::iam::operation_of(&parts.method, matched) {
            Some((_, Access::Org(permission))) => {
                let d = grants.decide(permission);
                // V1 service accounts read, and never the access service.
                let service_ok = auth.service.is_none()
                    || (permission.ends_with(":read")
                        && !permission.starts_with("iam:")
                        && !permission.starts_with("iam."));
                if !service_ok {
                    refused = Some(permission);
                    Some(crate::services::iam::Decision::Deny(
                        crate::services::iam::DenyReason::Implicit,
                    ))
                } else {
                    if !d.allowed() {
                        refused = Some(permission);
                    }
                    Some(d)
                }
            }
            // Off the registry's org operations (sockets, internal routes):
            // humans only.
            _ if auth.service.is_some() => return Err(access_key_not_allowed()),
            _ => None,
        };
        // Recorded before refusing: refusals are audited (D294).
        if let Some(slot) = parts
            .extensions
            .get::<crate::services::iam::observe::Slot>()
        {
            let who = crate::services::iam::observe::Who {
                org_id: org.id,
                user_id: auth.user.id,
                service_account_id: auth.service.as_ref().map(|s| s.id),
            };
            slot.record(&role, decision, who);
        }
        if let Some(permission) = refused {
            return Err(access_denied(permission));
        }

        Ok(OrgContext {
            role,
            grants,
            org,
            auth,
        })
    }
}

/// 403 of a permission the caller's role does not grant (iam.md §8).
pub fn access_denied(permission: &str) -> loco_rs::Error {
    crate::controllers::coded_error(
        axum::http::StatusCode::FORBIDDEN,
        pnex_core::err_codes::ACCESS_DENIED,
        format!("your role does not grant the {permission} permission"),
        Some(serde_json::json!({ "permission": permission })),
    )
}

/// Authenticated platform administrator (D72). Orthogonal to org roles:
/// a platform admin is not implicitly a member of every organization.
pub struct PlatformAdmin(pub AuthUser);

impl FromRequestParts<AppContext> for PlatformAdmin {
    type Rejection = loco_rs::Error;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppContext,
    ) -> Result<Self, Self::Rejection> {
        let auth = AuthUser::from_request_parts(parts, state).await?;
        if !auth.user.platform_admin {
            return Err(loco_rs::Error::CustomError(
                axum::http::StatusCode::FORBIDDEN,
                loco_rs::controller::ErrorDetail::new(
                    pnex_core::err_codes::PLATFORM_ADMIN_REQUIRED,
                    "platform administrator role required".to_string(),
                ),
            ));
        }
        Ok(PlatformAdmin(auth))
    }
}
