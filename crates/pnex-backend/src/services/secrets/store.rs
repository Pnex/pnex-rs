//! Vault rows and usage tracking (secrets.md D110, D113, D114).
//!
//! Every function is scoped by owner (`org_id`, `None` = platform): a row
//! of another org is reported as not found. Values are only decrypted by
//! [`reveal`], for server-side consumers — never for an API response.

use pnex_core::{
    SecretConsumerKind, SecretFieldInput, SecretUsage, SECRET_NAME_MAX, SECRET_VALUE_MAX,
};
use sea_orm::{
    ActiveModelTrait, ColumnTrait, ConnectionTrait, DbErr, EntityTrait, QueryFilter, Set,
};
use uuid::Uuid;

use super::crypto::{Keyring, KeyringError, Sealed, SecretCryptoError};
use crate::models::_entities::{
    flows, geo_providers, llm_providers, media_streams, notify_channels, org_secrets,
    secret_usages, wifi_credentials,
};
use crate::services::db_lock::is_unique_violation;

#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error("secret not found")]
    NotFound,
    #[error("secret is in use")]
    InUse,
    #[error("secret name already taken")]
    NameTaken,
    /// Field-level validation token (`required`, `max_length:255`).
    #[error("invalid field `{field}`: {token}")]
    Invalid { field: &'static str, token: String },
    #[error("owner or admin role required to write a secret")]
    WriteForbidden,
    /// R9: only owner/admin bind a secret to a field and a destination.
    #[error("secret of field `{field}` is bound to its destination (owner or admin required)")]
    DestinationLocked { field: String },
    #[error(transparent)]
    Keyring(#[from] KeyringError),
    #[error(transparent)]
    Crypto(#[from] SecretCryptoError),
    #[error(transparent)]
    Db(#[from] DbErr),
}

/// Who writes: the owner of the row and the acting user.
#[derive(Clone, Copy, Debug)]
pub struct Writer {
    pub org_id: Option<i64>,
    pub user_id: Option<i64>,
}

fn validate_name(name: &str) -> Result<String, StoreError> {
    let name = name.trim();
    if name.is_empty() {
        return Err(StoreError::Invalid {
            field: "name",
            token: pnex_core::err_codes::FIELD_REQUIRED.into(),
        });
    }
    if name.chars().count() > SECRET_NAME_MAX {
        return Err(StoreError::Invalid {
            field: "name",
            token: format!(
                "{}:{SECRET_NAME_MAX}",
                pnex_core::err_codes::FIELD_MAX_LENGTH
            ),
        });
    }
    Ok(name.to_string())
}

fn validate_value(value: &str) -> Result<(), StoreError> {
    if value.is_empty() {
        return Err(StoreError::Invalid {
            field: "value",
            token: pnex_core::err_codes::FIELD_REQUIRED.into(),
        });
    }
    if value.len() > SECRET_VALUE_MAX {
        return Err(StoreError::Invalid {
            field: "value",
            token: format!(
                "{}:{SECRET_VALUE_MAX}",
                pnex_core::err_codes::FIELD_MAX_LENGTH
            ),
        });
    }
    Ok(())
}

fn clean_description(description: Option<&str>) -> Option<String> {
    description
        .map(str::trim)
        .filter(|d| !d.is_empty())
        .map(str::to_string)
}

fn owner_filter(org_id: Option<i64>) -> sea_orm::Condition {
    match org_id {
        Some(id) => sea_orm::Condition::all().add(org_secrets::Column::OrgId.eq(id)),
        None => sea_orm::Condition::all().add(org_secrets::Column::OrgId.is_null()),
    }
}

fn map_unique(e: DbErr) -> StoreError {
    if is_unique_violation(&e) {
        StoreError::NameTaken
    } else {
        StoreError::Db(e)
    }
}

/// Row `id` of `org_id`, or `NotFound`.
pub async fn find<C: ConnectionTrait>(
    db: &C,
    org_id: Option<i64>,
    id: Uuid,
) -> Result<org_secrets::Model, StoreError> {
    org_secrets::Entity::find_by_id(id)
        .filter(owner_filter(org_id))
        .one(db)
        .await?
        .ok_or(StoreError::NotFound)
}

/// Row named `name` of `org_id`, if any.
pub async fn find_by_name<C: ConnectionTrait>(
    db: &C,
    org_id: Option<i64>,
    name: &str,
) -> Result<Option<org_secrets::Model>, StoreError> {
    Ok(org_secrets::Entity::find()
        .filter(owner_filter(org_id))
        .filter(org_secrets::Column::Name.eq(name.trim()))
        .one(db)
        .await?)
}

/// Creates a secret holding `value`.
pub async fn create<C: ConnectionTrait>(
    db: &C,
    ring: &Keyring,
    writer: Writer,
    name: &str,
    description: Option<&str>,
    value: &str,
) -> Result<org_secrets::Model, StoreError> {
    let name = validate_name(name)?;
    validate_value(value)?;
    let id = Uuid::new_v4();
    let sealed = ring.seal(writer.org_id, id, value);
    let now: sea_orm::prelude::DateTimeWithTimeZone = chrono::Utc::now().into();
    org_secrets::ActiveModel {
        id: Set(id),
        org_id: Set(writer.org_id),
        name: Set(name),
        description: Set(clean_description(description)),
        ciphertext: Set(sealed.ciphertext),
        nonce: Set(sealed.nonce),
        key_id: Set(sealed.key_id),
        created_by: Set(writer.user_id),
        updated_by: Set(writer.user_id),
        created_at: Set(now),
        updated_at: Set(now),
    }
    .insert(db)
    .await
    .map_err(map_unique)
}

/// Renames / redescribes a secret and, when `value` is given, replaces it.
pub async fn update<C: ConnectionTrait>(
    db: &C,
    ring: &Keyring,
    writer: Writer,
    id: Uuid,
    name: &str,
    description: Option<&str>,
    value: Option<&str>,
) -> Result<org_secrets::Model, StoreError> {
    let name = validate_name(name)?;
    if let Some(v) = value {
        validate_value(v)?;
    }
    let row = find(db, writer.org_id, id).await?;
    let mut am: org_secrets::ActiveModel = row.into();
    am.name = Set(name);
    am.description = Set(clean_description(description));
    if let Some(v) = value {
        let sealed = ring.seal(writer.org_id, id, v);
        am.ciphertext = Set(sealed.ciphertext);
        am.nonce = Set(sealed.nonce);
        am.key_id = Set(sealed.key_id);
    }
    am.updated_by = Set(writer.user_id);
    am.update(db).await.map_err(map_unique)
}

/// Deletes an unused secret (`InUse` otherwise, D114).
pub async fn delete<C: ConnectionTrait>(
    db: &C,
    org_id: Option<i64>,
    id: Uuid,
) -> Result<(), StoreError> {
    find(db, org_id, id).await?;
    let used = secret_usages::Entity::find()
        .filter(secret_usages::Column::SecretId.eq(id))
        .one(db)
        .await?
        .is_some();
    if used {
        return Err(StoreError::InUse);
    }
    org_secrets::Entity::delete_by_id(id).exec(db).await?;
    Ok(())
}

/// Decrypts a secret for a server-side consumer. Never return this to a
/// client and never log it.
pub async fn reveal<C: ConnectionTrait>(
    db: &C,
    ring: &Keyring,
    org_id: Option<i64>,
    id: Uuid,
) -> Result<String, StoreError> {
    let row = find(db, org_id, id).await?;
    Ok(ring.open(
        row.org_id,
        row.id,
        &Sealed {
            ciphertext: row.ciphertext,
            nonce: row.nonce,
            key_id: row.key_id,
        },
    )?)
}

/// Resolves a secret field of a functional form (D113) to a vault id.
///
/// - `Pick`: the secret must belong to the org.
/// - `Value`: owner/admin only; replaces the dedicated secret
///   `dedicated_name` if it exists, else creates it.
pub async fn resolve_field<C: ConnectionTrait>(
    db: &C,
    ring: &Keyring,
    writer: Writer,
    can_write_secrets: bool,
    input: &SecretFieldInput,
    dedicated_name: &str,
) -> Result<Uuid, StoreError> {
    match input {
        SecretFieldInput::Pick { secret_id } => Ok(find(db, writer.org_id, *secret_id).await?.id),
        SecretFieldInput::Value { value } => {
            if !can_write_secrets {
                return Err(StoreError::WriteForbidden);
            }
            match find_by_name(db, writer.org_id, dedicated_name).await? {
                Some(row) => {
                    let desc = row.description.clone();
                    Ok(update(
                        db,
                        ring,
                        writer,
                        row.id,
                        &row.name,
                        desc.as_deref(),
                        Some(value),
                    )
                    .await?
                    .id)
                }
                None => Ok(create(db, ring, writer, dedicated_name, None, value)
                    .await?
                    .id),
            }
        }
    }
}

/// A consumer holding a single secret field (WiFi entry, LLM provider).
pub struct SingleConsumer<'a> {
    pub kind: SecretConsumerKind,
    pub id: &'a str,
    pub field: &'a str,
    /// Name prefix of its dedicated secret (`wifi/<ssid>/`).
    pub prefix: String,
    /// Prefix before a rename of the consumer: the dedicated secret
    /// follows it.
    pub previous_prefix: Option<String>,
}

impl SingleConsumer<'_> {
    fn dedicated(&self, prefix: &str) -> String {
        format!("{prefix}{}", self.field)
    }
}

/// Stores the secret field of a [`SingleConsumer`] and rewrites its usage.
/// `input = None` keeps `current`; a dedicated secret no longer referenced
/// is deleted. Returns the secret now referenced.
pub async fn save_single<C: ConnectionTrait>(
    db: &C,
    ring: &Keyring,
    writer: Writer,
    can_write_secrets: bool,
    consumer: &SingleConsumer<'_>,
    current: Option<Uuid>,
    input: Option<&SecretFieldInput>,
) -> Result<Option<Uuid>, StoreError> {
    let name = consumer.dedicated(&consumer.prefix);
    // Strict R9 (SEC-W2): without secrets management, a single consumer
    // keeps its secret as is. Picking another one, or renaming the
    // consumer (the WiFi SSID is where the password goes) while it holds
    // one, is refused.
    if !can_write_secrets {
        let picks_other = matches!(
            input,
            Some(SecretFieldInput::Pick { secret_id }) if Some(*secret_id) != current
        );
        let renamed = current.is_some()
            && consumer
                .previous_prefix
                .as_deref()
                .is_some_and(|o| o != consumer.prefix);
        if picks_other || renamed {
            return Err(StoreError::DestinationLocked {
                field: consumer.field.to_string(),
            });
        }
    }
    if let (Some(old), Some(secret)) = (
        consumer
            .previous_prefix
            .as_deref()
            .filter(|o| *o != consumer.prefix),
        current,
    ) {
        if let Ok(row) = find(db, writer.org_id, secret).await {
            if row.name == consumer.dedicated(old) {
                rename(db, writer.org_id, secret, &name).await?;
            }
        }
    }
    let next = match input {
        None => current,
        Some(input) => {
            Some(resolve_field(db, ring, writer, can_write_secrets, input, &name).await?)
        }
    };
    let refs: Vec<(String, Uuid)> = next
        .map(|s| (consumer.field.to_string(), s))
        .into_iter()
        .collect();
    set_usages(db, consumer.kind, consumer.id, &refs).await?;
    if let Some(old) = current.filter(|c| Some(*c) != next) {
        prune_dedicated(db, writer.org_id, &[old], &consumer.prefix).await?;
    }
    Ok(next)
}

/// Rewrites the usages of one consumer (D114): `refs` = `(field, secret)`.
pub async fn set_usages<C: ConnectionTrait>(
    db: &C,
    kind: SecretConsumerKind,
    consumer_id: &str,
    refs: &[(String, Uuid)],
) -> Result<(), StoreError> {
    secret_usages::Entity::delete_many()
        .filter(secret_usages::Column::ConsumerKind.eq(kind.as_str()))
        .filter(secret_usages::Column::ConsumerId.eq(consumer_id))
        .exec(db)
        .await?;
    if refs.is_empty() {
        return Ok(());
    }
    let now: sea_orm::prelude::DateTimeWithTimeZone = chrono::Utc::now().into();
    let rows = refs
        .iter()
        .map(|(field, secret_id)| secret_usages::ActiveModel {
            secret_id: Set(*secret_id),
            consumer_kind: Set(kind.as_str().to_string()),
            consumer_id: Set(consumer_id.to_string()),
            field: Set(field.clone()),
            created_at: Set(now),
            updated_at: Set(now),
            ..Default::default()
        });
    secret_usages::Entity::insert_many(rows).exec(db).await?;
    Ok(())
}

/// Drops the usages of a deleted consumer, then deletes the secrets that
/// were dedicated to it (name under `dedicated_prefix`) and have no other
/// usage (D114).
pub async fn release_consumer<C: ConnectionTrait>(
    db: &C,
    org_id: Option<i64>,
    kind: SecretConsumerKind,
    consumer_id: &str,
    dedicated_prefix: &str,
) -> Result<(), StoreError> {
    let held: Vec<Uuid> = secret_usages::Entity::find()
        .filter(secret_usages::Column::ConsumerKind.eq(kind.as_str()))
        .filter(secret_usages::Column::ConsumerId.eq(consumer_id))
        .all(db)
        .await?
        .into_iter()
        .map(|u| u.secret_id)
        .collect();
    set_usages(db, kind, consumer_id, &[]).await?;
    prune_dedicated(db, org_id, &held, dedicated_prefix).await
}

/// Deletes, among `ids`, the secrets named under `dedicated_prefix` that
/// no consumer uses any more (a dedicated secret dropped by its consumer,
/// D114). Shared secrets and still-used ones are kept.
pub async fn prune_dedicated<C: ConnectionTrait>(
    db: &C,
    org_id: Option<i64>,
    ids: &[Uuid],
    dedicated_prefix: &str,
) -> Result<(), StoreError> {
    for id in ids {
        let Ok(row) = find(db, org_id, *id).await else {
            continue;
        };
        if !row.name.starts_with(dedicated_prefix) {
            continue;
        }
        match delete(db, org_id, *id).await {
            Ok(()) | Err(StoreError::InUse) | Err(StoreError::NotFound) => {}
            Err(e) => return Err(e),
        }
    }
    Ok(())
}

/// Renames a secret, keeping its value (dedicated secret following its
/// consumer's rename).
pub async fn rename<C: ConnectionTrait>(
    db: &C,
    org_id: Option<i64>,
    id: Uuid,
    name: &str,
) -> Result<(), StoreError> {
    let name = validate_name(name)?;
    let row = find(db, org_id, id).await?;
    let mut am: org_secrets::ActiveModel = row.into();
    am.name = Set(name);
    am.update(db).await.map_err(map_unique)?;
    Ok(())
}

/// Names of the given secrets of `org_id` (the "set · name" of a form).
pub async fn names_of<C: ConnectionTrait>(
    db: &C,
    org_id: Option<i64>,
    ids: &[Uuid],
) -> Result<std::collections::HashMap<Uuid, String>, StoreError> {
    if ids.is_empty() {
        return Ok(Default::default());
    }
    Ok(org_secrets::Entity::find()
        .filter(owner_filter(org_id))
        .filter(org_secrets::Column::Id.is_in(ids.iter().copied()))
        .all(db)
        .await?
        .into_iter()
        .map(|r| (r.id, r.name))
        .collect())
}

/// Usages of the given secrets, grouped by secret.
pub async fn usages_of<C: ConnectionTrait>(
    db: &C,
    ids: &[Uuid],
) -> Result<std::collections::HashMap<Uuid, Vec<SecretUsage>>, StoreError> {
    let mut out: std::collections::HashMap<Uuid, Vec<SecretUsage>> = Default::default();
    if ids.is_empty() {
        return Ok(out);
    }
    let rows = secret_usages::Entity::find()
        .filter(secret_usages::Column::SecretId.is_in(ids.iter().copied()))
        .all(db)
        .await?;
    for u in rows {
        let Some(kind) = SecretConsumerKind::parse(&u.consumer_kind) else {
            continue;
        };
        out.entry(u.secret_id).or_default().push(SecretUsage {
            kind,
            consumer_id: u.consumer_id,
            field: u.field,
            label: None,
        });
    }
    resolve_labels(db, &mut out).await?;
    for list in out.values_mut() {
        list.sort_by(|a, b| {
            (a.kind.as_str(), &a.consumer_id, &a.field).cmp(&(
                b.kind.as_str(),
                &b.consumer_id,
                &b.field,
            ))
        });
    }
    Ok(out)
}

/// Fills `label` with the consumer's display name (provider/channel/flow
/// name, WiFi SSID). One query per kind; an unresolvable id keeps `None`.
async fn resolve_labels<C: ConnectionTrait>(
    db: &C,
    usages: &mut std::collections::HashMap<Uuid, Vec<SecretUsage>>,
) -> Result<(), DbErr> {
    use std::collections::HashMap;
    let ids_of = |kind: SecretConsumerKind| -> Vec<String> {
        usages
            .values()
            .flatten()
            .filter(|u| u.kind == kind)
            .map(|u| u.consumer_id.clone())
            .collect()
    };
    let uuids = |ids: Vec<String>| -> Vec<Uuid> {
        ids.iter().filter_map(|s| Uuid::parse_str(s).ok()).collect()
    };
    let ints =
        |ids: Vec<String>| -> Vec<i64> { ids.iter().filter_map(|s| s.parse().ok()).collect() };

    let mut names: HashMap<(SecretConsumerKind, String), String> = HashMap::new();

    let ids = uuids(ids_of(SecretConsumerKind::LlmProvider));
    if !ids.is_empty() {
        for r in llm_providers::Entity::find()
            .filter(llm_providers::Column::Id.is_in(ids))
            .all(db)
            .await?
        {
            names.insert((SecretConsumerKind::LlmProvider, r.id.to_string()), r.name);
        }
    }
    let ids = uuids(ids_of(SecretConsumerKind::GeoProvider));
    if !ids.is_empty() {
        for r in geo_providers::Entity::find()
            .filter(geo_providers::Column::Id.is_in(ids))
            .all(db)
            .await?
        {
            names.insert((SecretConsumerKind::GeoProvider, r.id.to_string()), r.name);
        }
    }
    let ids = uuids(ids_of(SecretConsumerKind::MediaStream));
    if !ids.is_empty() {
        for r in media_streams::Entity::find()
            .filter(media_streams::Column::Id.is_in(ids))
            .all(db)
            .await?
        {
            names.insert((SecretConsumerKind::MediaStream, r.id.to_string()), r.name);
        }
    }
    let ids = uuids(ids_of(SecretConsumerKind::NotifyChannel));
    if !ids.is_empty() {
        for r in notify_channels::Entity::find()
            .filter(notify_channels::Column::Id.is_in(ids))
            .all(db)
            .await?
        {
            names.insert(
                (SecretConsumerKind::NotifyChannel, r.id.to_string()),
                r.name,
            );
        }
    }
    let ids = ints(ids_of(SecretConsumerKind::Flow));
    if !ids.is_empty() {
        for r in flows::Entity::find()
            .filter(flows::Column::Id.is_in(ids))
            .all(db)
            .await?
        {
            names.insert((SecretConsumerKind::Flow, r.id.to_string()), r.name);
        }
    }
    let ids = ints(ids_of(SecretConsumerKind::Wifi));
    if !ids.is_empty() {
        for r in wifi_credentials::Entity::find()
            .filter(wifi_credentials::Column::Id.is_in(ids))
            .all(db)
            .await?
        {
            names.insert((SecretConsumerKind::Wifi, r.id.to_string()), r.ssid);
        }
    }

    for u in usages.values_mut().flatten() {
        u.label = names.get(&(u.kind, u.consumer_id.clone())).cloned();
    }
    Ok(())
}
