//! Bail de vie par device (D9) — remplace la clé Redis `device:ping` Django.
//!
//! `device_states.last_seen_at` est rafraîchi à chaque frame valide
//! (throttlé par l'appelant, ~1 s) ; le bail est tenu tant qu'une session
//! est ouverte (map en-process, cf. `ws_ingest`) ou que `last_seen_at` est
//! frais. Le reaper (`deactivate_stale`) est l'unique écrivain de
//! `device_registries.active` — parité Celery Django (`handle_sensors` :
//! frais → true, périmé/absent → false), qui ne touchait jamais `active`
//! depuis le consumer WS.

use chrono::{DateTime, FixedOffset, TimeDelta, Utc};
use loco_rs::prelude::*;
use sea_orm::{sea_query::OnConflict, ColumnTrait, ConnectionTrait, EntityTrait, QueryFilter, Set};

use crate::models::_entities::device_states;
use crate::services::settings::IngestSettings;

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

/// Reaper (parité `handle_sensors` Django, l'unique écrivain de `active`) :
/// state frais → `active=true` ; actif sans state frais (silence, absence
/// de ligne) → `active=false`. Nettoie aussi les `connected` périmés des
/// sessions mortes sans close (crash). Retourne (activés, désactivés).
///
/// SQL dialect-free : le cutoff est calculé côté Rust puis bindé
/// (`Statement::from_sql_and_values` rend `$1`/`?` selon le backend) —
/// `interval` est du PG pur. ⚠ Ne jamais laisser `last_seen_at` au default
/// DB côté sqlite : le format `CURRENT_TIMESTAMP` diffère du RFC3339 bindé
/// par sea-orm (comparaison lexicographique faussée) — l'app le `Set()`
/// toujours explicitement (`touch`).
pub async fn deactivate_stale(
    db: &DatabaseConnection,
    silence_ttl_secs: i64,
) -> Result<(u64, u64)> {
    use sea_orm::Statement;
    let cutoff: DateTime<FixedOffset> = (Utc::now() - TimeDelta::seconds(silence_ttl_secs)).into();
    let backend = db.get_database_backend();
    let on = db
        .execute_raw(Statement::from_sql_and_values(
            backend,
            "UPDATE device_registries d SET active = true \
             WHERE NOT d.active AND EXISTS (SELECT 1 FROM device_states s \
             WHERE s.device_registry_id = d.id AND s.last_seen_at > $1)",
            [cutoff.into()],
        ))
        .await
        .map_err(|_| Error::InternalServerError)?
        .rows_affected();
    let off = db
        .execute_raw(Statement::from_sql_and_values(
            backend,
            "UPDATE device_registries d SET active = false \
             WHERE d.active AND NOT EXISTS (SELECT 1 FROM device_states s \
             WHERE s.device_registry_id = d.id AND s.last_seen_at > $1)",
            [cutoff.into()],
        ))
        .await
        .map_err(|_| Error::InternalServerError)?
        .rows_affected();
    let _ = db
        .execute_raw(Statement::from_sql_and_values(
            backend,
            "UPDATE device_states SET connected = false \
             WHERE connected AND last_seen_at <= $1",
            [cutoff.into()],
        ))
        .await
        .map_err(|_| Error::InternalServerError)?;
    Ok((on, off))
}

/// Tâche de fond du reaper, lancée au boot (`after_routes`) dans tout mode
/// serveur — `loco start` sans flag est `ServerOnly` (connect_workers
/// n'est PAS appelé), le reaper doit vivre dans le process serveur.
/// Skip uniquement en test (`ForegroundBlocking`), où la logique est
/// appelée directement. Détachée : vit tant que le runtime.
pub fn spawn_reaper(ctx: &AppContext) {
    use loco_rs::config::WorkerMode;
    if matches!(ctx.config.workers.mode, WorkerMode::ForegroundBlocking) {
        return;
    }
    let settings = IngestSettings::from_config(&ctx.config);
    let db = ctx.db.clone();
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(std::time::Duration::from_secs(
            settings.reaper_interval_secs,
        ));
        loop {
            tick.tick().await;
            match deactivate_stale(&db, settings.silence_ttl_secs).await {
                Ok((on, off)) if on + off > 0 => {
                    tracing::info!(
                        devices_activees = on,
                        devices_desactives = off,
                        "reaper liveness"
                    );
                }
                Ok(_) => {}
                Err(_) => tracing::warn!("reaper liveness : échec, retry au prochain tick"),
            }
        }
    });
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
