//! JIT provisioning (parity with the legacy `_get_or_create_user`, extended multi-tenant):
//! à la première requête authentifiée d'un utilisateur IdP inconnu, on
//! crée en une transaction :
//!
//! 1. `users` (idp_sub = `sub` de l'IdP, email, full_name)
//! 2. `user_profiles` (default values — equivalent of the legacy signal)
//! 3. son **organisation personnelle** (owner) sur le tier **Free**
//!    (équivalent multi-tenant du signal UserProfile)
//!
//! Un utilisateur déjà connu est resynchronisé si son email/nom change côté
//! Rauthy. Idempotent et sûr en concurrence (re-vérification dans la tx).

use sea_orm::{
    ActiveModelTrait, ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter, Set,
    TransactionTrait,
};

use crate::models::_entities::{
    organization_members, organizations,
    sea_orm_active_enums::{OrgMemberRole, UiTheme},
    user_profiles, users,
};

use super::claims::Claims;

#[derive(Debug, thiserror::Error)]
pub enum ProvisionError {
    #[error("le token ne contient pas d'email — requis à la première connexion")]
    MissingEmail,
    #[error(transparent)]
    Db(#[from] sea_orm::DbErr),
}

/// Trouve l'utilisateur par `idp_sub`, ou le crée avec son profil et
/// son org personnelle owner (tier Free).
///
/// Then promotes the user to platform admin when their email is listed in
/// `PNEX_PLATFORM_ADMIN_EMAILS` (the flag is never revoked automatically:
/// the database stays the source of truth).
pub async fn get_or_create_user(
    db: &DatabaseConnection,
    claims: &Claims,
) -> Result<users::Model, ProvisionError> {
    let user = find_or_create_user(db, claims).await?;
    ensure_platform_admin(db, user).await
}

/// Lowercased emails from `PNEX_PLATFORM_ADMIN_EMAILS` (comma separated),
/// read once per process.
fn platform_admin_emails() -> &'static [String] {
    static EMAILS: std::sync::OnceLock<Vec<String>> = std::sync::OnceLock::new();
    EMAILS.get_or_init(|| {
        parse_admin_emails(&std::env::var("PNEX_PLATFORM_ADMIN_EMAILS").unwrap_or_default())
    })
}

fn parse_admin_emails(raw: &str) -> Vec<String> {
    raw.split(',')
        .map(|e| e.trim().to_lowercase())
        .filter(|e| !e.is_empty())
        .collect()
}

async fn ensure_platform_admin(
    db: &DatabaseConnection,
    user: users::Model,
) -> Result<users::Model, ProvisionError> {
    if user.platform_admin {
        return Ok(user);
    }
    let email = user.email.to_lowercase();
    if !platform_admin_emails().iter().any(|e| *e == email) {
        return Ok(user);
    }
    let mut active: users::ActiveModel = user.into();
    active.platform_admin = Set(true);
    let promoted = active.update(db).await?;
    tracing::info!(
        user_id = promoted.id,
        "user promoted to platform admin (PNEX_PLATFORM_ADMIN_EMAILS)"
    );
    Ok(promoted)
}

async fn find_or_create_user(
    db: &DatabaseConnection,
    claims: &Claims,
) -> Result<users::Model, ProvisionError> {
    let idp_sub = claims.sub.clone();

    if let Some(user) = users::Entity::find()
        .filter(users::Column::IdpSub.eq(&idp_sub))
        .one(db)
        .await?
    {
        return sync_user(db, user, claims).await;
    }

    let email = claims.email.clone().ok_or(ProvisionError::MissingEmail)?;
    let full_name = claims.display_name();

    db.transaction(|txn| {
        Box::pin(async move {
            // Re-vérification dans la transaction : deux requêtes simultanées du
            // même nouvel utilisateur ne doivent produire qu'une ligne users.
            if let Some(existing) = users::Entity::find()
                .filter(users::Column::IdpSub.eq(&idp_sub))
                .one(txn)
                .await?
            {
                return Ok(existing);
            }

            // Liaison par email : même personne avec un `sub` inconnu (IdP
            // migré — cas concret : Keycloak → Rauthy, les subs changent ; ou
            // realm réimporté). `users.email` est unique : on RE-LIE la ligne
            // existante au lieu d'insérer un doublon (qui violerait la contrainte).
            if let Some(existing) = users::Entity::find()
                .filter(users::Column::Email.eq(&email))
                .one(txn)
                .await?
            {
                let mut active: users::ActiveModel = existing.into();
                active.idp_sub = Set(Some(idp_sub.clone()));
                if !full_name.is_empty() {
                    active.full_name = Set(Some(full_name.clone()));
                }
                let relinked = active.update(txn).await?;
                tracing::info!(
                    user_id = relinked.id,
                    "utilisateur re-lie par email (sub IdP change, migration Keycloak→Rauthy)"
                );
                return Ok(relinked);
            }

            let user = users::ActiveModel {
                idp_sub: Set(Some(idp_sub.clone())),
                email: Set(email),
                full_name: Set(Some(full_name.clone())),
                ..Default::default()
            }
            .insert(txn)
            .await?;

            user_profiles::ActiveModel {
                user_id: Set(user.id),
                language: Set("en".into()),
                timezone: Set("UTC".into()),
                theme: Set(UiTheme::Light),
                ..Default::default()
            }
            .insert(txn)
            .await?;

            create_personal_org(txn, &user, &full_name).await?;

            Ok(user)
        })
    })
    .await
    .map_err(|err| match err {
        sea_orm::TransactionError::Connection(db_err) => ProvisionError::Db(db_err),
        sea_orm::TransactionError::Transaction(callback_err) => callback_err,
    })
}

/// Resyncs email/name from the IdP when they diverge (legacy parity).
async fn sync_user(
    db: &DatabaseConnection,
    user: users::Model,
    claims: &Claims,
) -> Result<users::Model, ProvisionError> {
    let email = claims.email.clone().unwrap_or_default();
    let full_name = claims.display_name();
    if user.email != email || user.full_name.as_deref() != Some(full_name.as_str()) {
        let mut active: users::ActiveModel = user.into();
        if !email.is_empty() {
            active.email = Set(email);
        }
        active.full_name = Set(Some(full_name));
        return Ok(active.update(db).await?);
    }
    Ok(user)
}

/// Org personnelle : tier par défaut (`PNEX_DEFAULT_ORG_TIER`, dev : Admin
/// pour débrider le compte de test — Free sinon), l'utilisateur en est owner.
async fn create_personal_org<C>(
    db: &C,
    user: &users::Model,
    display: &str,
) -> Result<(), sea_orm::DbErr>
where
    C: sea_orm::ConnectionTrait,
{
    let tier_id = crate::controllers::orgs::default_org_tier(db).await;
    // Self-hosted deployments have no tiers by design: only warn in SaaS.
    if tier_id.is_none()
        && crate::services::retention::DeploymentMode::from_env()
            == crate::services::retention::DeploymentMode::Saas
    {
        tracing::warn!(
            user_id = user.id,
            "no subscription tier found (seed not run?): personal org created without tier"
        );
    }

    let base = format!("Organisation de {display}");
    // `organizations.name` est unique : on suffixe par l'id user en cas de
    // collision (deux « Alice Martin » peuvent exister).
    let name = match organizations::Entity::find()
        .filter(organizations::Column::Name.eq(&base))
        .one(db)
        .await?
    {
        Some(_) => format!("{base} #{}", user.id),
        None => base,
    };

    let org = organizations::ActiveModel {
        name: Set(name),
        subscription_tier_id: Set(tier_id),
        ..Default::default()
    }
    .insert(db)
    .await?;

    organization_members::ActiveModel {
        org_id: Set(org.id),
        user_id: Set(user.id),
        role: Set(OrgMemberRole::Owner),
        ..Default::default()
    }
    .insert(db)
    .await?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::parse_admin_emails;

    #[test]
    fn admin_emails_are_trimmed_lowercased_and_non_empty() {
        assert_eq!(
            parse_admin_emails(" Admin@Example.com, ,ops@pnex.io "),
            vec!["admin@example.com".to_string(), "ops@pnex.io".to_string()]
        );
        assert!(parse_admin_emails("").is_empty());
    }
}
