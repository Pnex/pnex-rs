//! Bail de vie par device (D9) — remplace la clé Redis `device:ping` Django.
//!
//! `device_states.last_seen_at` est rafraîchi à chaque frame valide
//! (throttlé par l'appelant, ~1 s) ; le bail est tenu tant qu'une session
//! est ouverte (map en-process, cf. `ws_ingest`) ou que `last_seen_at` est
//! frais. Le reaper (`deactivate_stale`) est l'unique écrivain de
//! `device_registries.active` — parité Celery Django, qui ne touchait
//! jamais `active` depuis le consumer WS.

use chrono::{DateTime, TimeDelta, Utc};
use loco_rs::prelude::*;
use sea_orm::{ColumnTrait, EntityTrait, QueryFilter, Set, sea_query::OnConflict};

use crate::models::_entities::device_states;

/// `last_seen` encore frais au sens du TTL de silence ?
pub fn is_fresh(last_seen: DateTime<Utc>, silence_ttl_secs: i64) -> bool {
    last_seen + TimeDelta::seconds(silence_ttl_secs) > Utc::now()
}

/// Upsert du last_seen ; `connected` mis à jour seulement si fourni
/// (true à la prise de bail, false à la déconnexion propre).
pub async fn touch(
    db: &DatabaseConnection,
    device_registry_id: i64,
    connected: Option<bool>,
) -> Result<()> {
    let mut conflict = OnConflict::column(device_states::Column::DeviceRegistryId)
        .update_column(device_states::Column::LastSeenAt)
        .to_owned();
    if let Some(c) = connected {
        conflict.value(device_states::Column::Connected, c);
    }
    device_states::Entity::insert(device_states::ActiveModel {
        device_registry_id: Set(device_registry_id),
        last_seen_at: Set(Utc::now().into()),
        connected: Set(connected.unwrap_or(false)),
        ..Default::default()
    })
    .on_conflict(conflict)
    .exec(db)
    .await
    .map_err(|_| Error::InternalServerError)?;
    Ok(())
}

/// Déconnexion propre : last_seen honnête (dernier instant de vie) + bail
/// libéré (`connected=false`) — un reconnect immédiat est accepté, là où
/// Django gardait la fenêtre de 12 s sur la clé ping.
pub async fn release(db: &DatabaseConnection, device_registry_id: i64) -> Result<()> {
    touch(db, device_registry_id, Some(false)).await
}

/// État de vie d'un device, si ligne existante.
pub async fn state_of(
    db: &DatabaseConnection,
    device_registry_id: i64,
) -> Result<Option<device_states::Model>> {
    device_states::Entity::find()
        .filter(device_states::Column::DeviceRegistryId.eq(device_registry_id))
        .one(db)
        .await
        .map_err(|_| Error::InternalServerError)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Limite TTL exacte : frais strictement avant, périmé après.
    #[test]
    fn fraicheur_bord_du_ttl() {
        let ttl = 10;
        let now = Utc::now();
        assert!(is_fresh(now - TimeDelta::seconds(ttl - 1), ttl));
        assert!(!is_fresh(now - TimeDelta::seconds(ttl), ttl));
    }
}
