//! JIT provisioning (parité Django `_get_or_create_user`, étendu multi-tenant) :
//! à la première requête authentifiée d'un utilisateur Keycloak inconnu, on
//! crée en une transaction :
//!
//! 1. `users` (keycloak_uuid = `sub`, email, full_name)
//! 2. `user_profiles` (valeurs par défaut — équivalent du signal Django)
//! 3. son **organisation personnelle** (owner) sur le tier **Free**
//!    (équivalent multi-tenant du signal UserProfile)
//!
//! Un utilisateur déjà connu est resynchronisé si son email/nom change côté
//! Keycloak. Idempotent et sûr en concurrence (re-vérification dans la tx).

use sea_orm::{
    ActiveModelTrait, ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter, Set,
    TransactionTrait,
};
use uuid::Uuid;

use crate::models::_entities::{
    organization_members, organizations, sea_orm_active_enums::{OrgMemberRole, UiTheme},
    subscription_tiers, user_profiles, users,
};

use super::claims::Claims;

#[derive(Debug, thiserror::Error)]
pub enum ProvisionError {
    #[error("le claim sub n'est pas un UUID valide")]
    InvalidSub,
    #[error("le token ne contient pas d'email — requis à la première connexion")]
    MissingEmail,
    #[error(transparent)]
    Db(#[from] sea_orm::DbErr),
}

/// Trouve l'utilisateur par `keycloak_uuid`, ou le crée avec son profil et
/// son org personnelle owner (tier Free).
pub async fn get_or_create_user(
    db: &DatabaseConnection,
    claims: &Claims,
) -> Result<users::Model, ProvisionError> {
    let kc_uuid =
        Uuid::parse_str(&claims.sub).map_err(|_| ProvisionError::InvalidSub)?;

    if let Some(user) = users::Entity::find()
        .filter(users::Column::KeycloakUuid.eq(kc_uuid))
        .one(db)
        .await?
    {
        return sync_user(db, user, claims).await;
    }

    let email = claims
        .email
        .clone()
        .ok_or(ProvisionError::MissingEmail)?;
    let full_name = claims.display_name();

    db.transaction(|txn| Box::pin(async move {
        // Re-vérification dans la transaction : deux requêtes simultanées du
        // même nouvel utilisateur ne doivent produire qu'une ligne users.
        if let Some(existing) = users::Entity::find()
            .filter(users::Column::KeycloakUuid.eq(kc_uuid))
            .one(txn)
            .await?
        {
            return Ok(existing);
        }

        // Liaison par email : même personne avec un `sub` inconnu (realm
        // Keycloak réimporté — les users du realm de dev n'ont pas d'id fixe,
        // migration d'IdP…). `users.email` est unique : on RE-LIE la ligne
        // existante au lieu d'insérer un doublon (qui violerait la contrainte).
        if let Some(existing) = users::Entity::find()
            .filter(users::Column::Email.eq(&email))
            .one(txn)
            .await?
        {
            let mut active: users::ActiveModel = existing.into();
            active.keycloak_uuid = Set(Some(kc_uuid));
            if !full_name.is_empty() {
                active.full_name = Set(Some(full_name.clone()));
            }
            let relinked = active.update(txn).await?;
            tracing::info!(user_id = relinked.id, "utilisateur re-lie par email (sub Keycloak change)");
            return Ok(relinked);
        }

        let user = users::ActiveModel {
            keycloak_uuid: Set(Some(kc_uuid)),
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
    }))
    .await
    .map_err(|err| match err {
        sea_orm::TransactionError::Connection(db_err) => ProvisionError::Db(db_err),
        sea_orm::TransactionError::Transaction(callback_err) => callback_err,
    })
}

/// Resynchronise email/nom depuis Keycloak si divergents (parité Django).
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

/// Org personnelle : tier Free, l'utilisateur en est owner.
async fn create_personal_org<C>(db: &C, user: &users::Model, display: &str) -> Result<(), sea_orm::DbErr>
where
    C: sea_orm::ConnectionTrait,
{
    let free_tier = subscription_tiers::Entity::find()
        .filter(subscription_tiers::Column::Name.eq("Free"))
        .one(db)
        .await?;
    if free_tier.is_none() {
        tracing::warn!(
            user_id = user.id,
            "tier Free introuvable (seed non exécuté ?) — org personnelle sans tier"
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
        subscription_tier_id: Set(free_tier.map(|t| t.id)),
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
