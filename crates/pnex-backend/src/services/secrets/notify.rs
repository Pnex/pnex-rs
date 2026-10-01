//! Notification channels as vault consumers (secrets.md lot S4, closes
//! notifications.md N5).
//!
//! The `secret` fields of a stored channel config hold
//! `{"secret_id": "…"}`. On save, each one is a choice (`{"secret_id"}`),
//! a typed value (`{"value"}` or a bare string — creates or replaces the
//! dedicated secret `notify/<channel>/<field>`), or `null`/absent (kept).
//! Sends resolve the references in memory: the backend with the keyring
//! ([`sendable`]), the flow runtime through `/internal/flow/secret/{id}`
//! gated by [`reachable_from_deployed_flows`].

use pnex_core::{FlowGraph, FlowNodeKind, SecretConsumerKind, SecretFieldInput};
use sea_orm::{ColumnTrait, ConnectionTrait, DatabaseConnection, EntityTrait, QueryFilter};
use uuid::Uuid;

use super::crypto::Keyring;
use super::store::{self, StoreError, Writer};
use crate::models::_entities::{notify_channels, secret_usages};

/// Name prefix of every secret dedicated to a channel.
pub fn dedicated_prefix(channel_name: &str) -> String {
    format!("notify/{channel_name}/")
}

fn dedicated_name(channel_name: &str, field: &str) -> String {
    format!("{}{field}", dedicated_prefix(channel_name))
}

/// One secret field of a channel being saved.
#[derive(Debug, Clone, PartialEq)]
enum Slot {
    /// Reference to an existing secret of the org.
    Ref(Uuid),
    /// Value typed in the form (owner/admin only).
    Typed(String),
    /// Plaintext left by a pre-vault config, carried over on save.
    Legacy(String),
}

/// A channel config checked against the vault, ready to validate then
/// commit. `sendable` holds the plaintext values: validation input only,
/// never stored nor returned.
#[derive(Debug)]
pub struct Plan {
    base: serde_json::Value,
    slots: Vec<(String, Slot)>,
    pub sendable: serde_json::Value,
}

/// Reads the secret fields of `merged` (the incoming config, after the
/// "null = unchanged" merge with `existing`) and resolves the plaintext of
/// each one for validation. A picked secret of another org is `NotFound`.
pub async fn plan<C: ConnectionTrait>(
    db: &C,
    ring: &Keyring,
    org_id: i64,
    kind: &str,
    existing: Option<&serde_json::Value>,
    merged: &serde_json::Value,
) -> Result<Plan, StoreError> {
    let mut base = merged.clone();
    let mut slots = Vec::new();
    let mut values = Vec::new();
    for field in pnex_notify::secrets::secret_fields(kind) {
        let incoming = merged.get(&field).cloned();
        if let Some(obj) = base.as_object_mut() {
            obj.remove(&field);
        }
        let slot = match incoming {
            None | Some(serde_json::Value::Null) => continue,
            Some(serde_json::Value::String(s)) if s.is_empty() => continue,
            Some(serde_json::Value::String(s)) => {
                let kept = existing
                    .and_then(|e| e.get(&field))
                    .and_then(|v| v.as_str());
                if kept == Some(s.as_str()) {
                    Slot::Legacy(s)
                } else {
                    Slot::Typed(s)
                }
            }
            Some(v) => match serde_json::from_value::<SecretFieldInput>(v) {
                Ok(SecretFieldInput::Pick { secret_id }) => Slot::Ref(secret_id),
                Ok(SecretFieldInput::Value { value }) if value.is_empty() => continue,
                Ok(SecretFieldInput::Value { value }) => Slot::Typed(value),
                Err(_) => {
                    return Err(StoreError::Invalid {
                        field: "config",
                        token: format!("invalid secret field: {field}"),
                    })
                }
            },
        };
        let plain = match &slot {
            Slot::Ref(id) => store::reveal(db, ring, Some(org_id), *id).await?,
            Slot::Typed(v) | Slot::Legacy(v) => v.clone(),
        };
        values.push((field.clone(), plain));
        slots.push((field, slot));
    }
    let sendable = pnex_notify::secrets::with_values(&base, &values);
    Ok(Plan {
        base,
        slots,
        sendable,
    })
}

/// Stores the secret fields of a validated [`Plan`] and rewrites the
/// channel's usages. `previous` = `(name, config)` of the channel before
/// this save: its dedicated secrets follow a rename and are deleted when
/// no longer referenced. Returns the config to store.
#[allow(clippy::too_many_arguments)] // two call sites, kept explicit
pub async fn commit<C: ConnectionTrait>(
    db: &C,
    ring: &Keyring,
    writer: Writer,
    can_write_secrets: bool,
    channel_id: Uuid,
    channel_name: &str,
    previous: Option<(&str, &str, &serde_json::Value)>,
    plan: Plan,
) -> Result<serde_json::Value, StoreError> {
    let held: Vec<(String, Uuid)> = previous
        .map(|(kind, _, cfg)| pnex_notify::secrets::secret_refs(kind, cfg))
        .unwrap_or_default();
    // Dedicated secrets follow the channel's rename (same field only).
    if let Some((_, old_name, _)) = previous.filter(|(_, n, _)| *n != channel_name) {
        for (field, id) in &held {
            let Ok(row) = store::find(db, writer.org_id, *id).await else {
                continue;
            };
            if row.name == dedicated_name(old_name, field) {
                store::rename(db, writer.org_id, *id, &dedicated_name(channel_name, field)).await?;
            }
        }
    }
    let mut refs = Vec::with_capacity(plan.slots.len());
    for (field, slot) in plan.slots {
        let id = match slot {
            Slot::Ref(id) => id,
            Slot::Typed(value) => {
                store::resolve_field(
                    db,
                    ring,
                    writer,
                    can_write_secrets,
                    &SecretFieldInput::Value { value },
                    &dedicated_name(channel_name, &field),
                )
                .await?
            }
            // Carrying an existing value over is not a new write.
            Slot::Legacy(value) => {
                store::resolve_field(
                    db,
                    ring,
                    writer,
                    true,
                    &SecretFieldInput::Value { value },
                    &dedicated_name(channel_name, &field),
                )
                .await?
            }
        };
        refs.push((field, id));
    }
    let consumer = channel_id.to_string();
    store::set_usages(db, SecretConsumerKind::NotifyChannel, &consumer, &refs).await?;
    let dropped: Vec<Uuid> = held
        .iter()
        .map(|(_, id)| *id)
        .filter(|id| !refs.iter().any(|(_, r)| r == id))
        .collect();
    store::prune_dedicated(db, writer.org_id, &dropped, &dedicated_prefix(channel_name)).await?;
    Ok(pnex_notify::secrets::with_refs(&plan.base, &refs))
}

/// Drops the usages of a deleted channel and its dedicated secrets.
pub async fn release<C: ConnectionTrait>(
    db: &C,
    org_id: i64,
    channel_id: Uuid,
    channel_name: &str,
) -> Result<(), StoreError> {
    store::release_consumer(
        db,
        Some(org_id),
        SecretConsumerKind::NotifyChannel,
        &channel_id.to_string(),
        &dedicated_prefix(channel_name),
    )
    .await
}

/// The channel config with its references replaced by their values
/// (backend sends: Test buttons). Memory only — never store nor return it.
pub async fn sendable<C: ConnectionTrait>(
    db: &C,
    ring: &Keyring,
    org_id: i64,
    kind: &str,
    config: &serde_json::Value,
) -> Result<serde_json::Value, StoreError> {
    let mut values = Vec::new();
    for (field, id) in pnex_notify::secrets::secret_refs(kind, config) {
        values.push((field, store::reveal(db, ring, Some(org_id), id).await?));
    }
    Ok(pnex_notify::secrets::with_values(config, &values))
}

/// `field → {secret_id, name}` of a channel config (read model of the
/// form). A dangling reference is left out.
pub async fn views<C: ConnectionTrait>(
    db: &C,
    org_id: i64,
    kind: &str,
    config: &serde_json::Value,
) -> Result<std::collections::BTreeMap<String, pnex_core::SecretFieldView>, StoreError> {
    let refs = pnex_notify::secrets::secret_refs(kind, config);
    let ids: Vec<Uuid> = refs.iter().map(|(_, id)| *id).collect();
    let names = store::names_of(db, Some(org_id), &ids).await?;
    Ok(refs
        .into_iter()
        .filter_map(|(field, id)| {
            let name = names.get(&id)?.clone();
            Some((
                field,
                pnex_core::SecretFieldView {
                    secret_id: id,
                    name,
                },
            ))
        })
        .collect())
}

/// D111/D115: may the flow runtime of `org_id` read `secret_id`? Only
/// when a deployed flow of the org references it: from an http-fetch node
/// field (lot S5), or through an enabled channel of the org that one of
/// its notify nodes sends on (lot S4).
pub async fn reachable_from_deployed_flows(
    db: &DatabaseConnection,
    org_id: i64,
    secret_id: Uuid,
) -> loco_rs::Result<bool> {
    let holders: Vec<Uuid> = secret_usages::Entity::find()
        .filter(secret_usages::Column::SecretId.eq(secret_id))
        .filter(secret_usages::Column::ConsumerKind.eq(SecretConsumerKind::NotifyChannel.as_str()))
        .all(db)
        .await
        .map_err(|_| loco_rs::Error::InternalServerError)?
        .into_iter()
        .filter_map(|u| Uuid::parse_str(&u.consumer_id).ok())
        .collect();
    let enabled: Vec<Uuid> = if holders.is_empty() {
        Vec::new()
    } else {
        notify_channels::Entity::find()
            .filter(notify_channels::Column::OrgId.eq(org_id))
            .filter(notify_channels::Column::Enabled.eq(true))
            .filter(notify_channels::Column::Id.is_in(holders))
            .all(db)
            .await
            .map_err(|_| loco_rs::Error::InternalServerError)?
            .into_iter()
            .map(|c| c.id)
            .collect()
    };
    for (_, version) in crate::controllers::flows::deployed_flows_with_versions(db, org_id).await? {
        let Ok(graph) = serde_json::from_value::<FlowGraph>(version.graph) else {
            continue;
        };
        if super::flow::graph_uses(&graph, secret_id) {
            return Ok(true);
        }
        let sends = graph.nodes.iter().any(|n| match &n.kind {
            FlowNodeKind::PnexNotify { config } => {
                config.channel_ids.iter().any(|c| enabled.contains(c))
            }
            _ => false,
        });
        if sends {
            return Ok(true);
        }
    }
    Ok(false)
}
