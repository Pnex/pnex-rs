//! Dashboard (2026-08-19) — lecture seule pour
//! `GET /api/v1/dashboard/summary` : liveness PG, stats builds PG,
//! dernières mesures OpenObserve.
//!
//! Doctrine : la branche télémétrie ne fait **jamais** échouer la requête
//! (org non provisionnée, O2 injoignable, timeout 3 s →
//! `telemetry.available == false`) et ne déclenche **jamais** de
//! provisioning (`provisioned_credentials`, lecture seule). Les sections
//! PG sont toujours servies.

use std::collections::HashMap;
use std::time::Duration;

use chrono::{DateTime, Utc};
use loco_rs::prelude::*;
use pnex_core::{BuildStats, DeviceLiveness, LatestMeasurement, LivenessSummary, TelemetrySummary};
use sea_orm::{ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter};

use crate::models::_entities::{
    build_records, device_registries, device_types, predefined_devices,
};
use crate::services::device_liveness;
use crate::services::openobserve::{self, client::Client};
use crate::services::visualization;

/// Fenêtre de recherche du dernier échantillon de chaque série.
const LATEST_WINDOW: &str = "1h";
/// [`LATEST_WINDOW`] in seconds (SQL last-sample lookup).
const LATEST_WINDOW_SECS: i64 = 3_600;

/// Timeout de TOUT le chemin O2 du summary (découverte des streams +
/// queries) — le dashboard répond quoi qu'il arrive, une page ne doit pas
/// traîner 10 s (timeout http du client) parce qu'O2 est lent.
const O2_TIMEOUT: Duration = Duration::from_secs(3);

/// Borné côté serveur : la table du dashboard reste lisible (demande user
/// 2026-08-19 — « only latest ~10 », pas tout l'historique).
const LATEST_CAP: usize = 10;

/// Idem pour la liste liveness : les ~10 devices les plus récemment
/// actifs (live d'abord) — les compteurs de la carte restent calculés
/// sur l'ensemble des devices de l'org.
const LIVENESS_CAP: usize = 10;

/// Nombre max de streams interrogés (défensif : sélecteur borné même si
/// une org a des dizaines de métriques dynamiques).
const STREAMS_CAP: usize = 12;

/// Liveness of the org's devices: registry × liveness store (Valkey +
/// Postgres cold record), fresh with respect to the silence TTL
/// (`device_liveness::is_fresh`, same definition as the reaper — not the
/// `active` flag, possibly stale between two ticks). Live first, then last
/// sign of life descending, **truncated to `LIVENESS_CAP`** — `total` /
/// `live` stay the full org counts.
pub async fn liveness(
    db: &DatabaseConnection,
    org_id: i64,
    silence_ttl_secs: i64,
) -> Result<LivenessSummary> {
    use sea_orm::{QueryOrder, QuerySelect};
    // Liveness lives in Valkey (D108): rank the org's devices in memory from
    // one batched read, then load only the LIVENESS_CAP freshest rows.
    // Fresh = `last_seen + ttl > now` (`device_liveness::is_fresh`).
    let ids: Vec<i64> = device_registries::Entity::find()
        .select_only()
        .column(device_registries::Column::Id)
        .filter(device_registries::Column::OrgId.eq(org_id))
        .order_by_desc(device_registries::Column::Id)
        .into_tuple::<i64>()
        .all(db)
        .await
        .map_err(|_| Error::InternalServerError)?;
    let seen = device_liveness::seen_many(db, &ids).await?;
    let last_seen_of = |id: &i64| seen.get(id).and_then(|s| s.last_seen);
    let total = ids.len() as u64;
    let live = ids
        .iter()
        .filter(|id| {
            last_seen_of(id).is_some_and(|t| device_liveness::is_fresh(t, silence_ttl_secs))
        })
        .count() as u64;
    // Live first then most recent sign of life == last_seen DESC (a live
    // device is by definition fresher than a silent one), never-seen last,
    // ties by id DESC (`ids` is already in that order, the sort is stable).
    let mut ranked = ids.clone();
    ranked.sort_by_key(|id| std::cmp::Reverse(last_seen_of(id)));
    ranked.truncate(LIVENESS_CAP);
    let mut by_id: HashMap<i64, device_registries::Model> = device_registries::Entity::find()
        .filter(device_registries::Column::Id.is_in(ranked.clone()))
        .all(db)
        .await
        .map_err(|_| Error::InternalServerError)?
        .into_iter()
        .map(|d| (d.id, d))
        .collect();
    let rows: Vec<(device_registries::Model, Option<DateTime<Utc>>)> = ranked
        .iter()
        .filter_map(|id| Some((by_id.remove(id)?, last_seen_of(id))))
        .collect();

    // predefined → (nom du modèle, type) pour l'affichage (page only).
    let pd_ids: Vec<i64> = rows.iter().map(|(d, _)| d.predefined_device_id).collect();
    let predefined: HashMap<i64, (String, i64)> = predefined_devices::Entity::find()
        .filter(predefined_devices::Column::Id.is_in(pd_ids))
        .all(db)
        .await
        .map_err(|_| Error::InternalServerError)?
        .into_iter()
        .map(|p| (p.id, (p.name, p.device_type_id)))
        .collect();
    let types: HashMap<i64, String> = device_types::Entity::find()
        .all(db)
        .await
        .map_err(|_| Error::InternalServerError)?
        .into_iter()
        .map(|t| (t.id, t.name))
        .collect();

    let devices: Vec<DeviceLiveness> = rows
        .into_iter()
        .map(|(device, seen_utc)| {
            let live = seen_utc.is_some_and(|t| device_liveness::is_fresh(t, silence_ttl_secs));
            let (name, type_id) = predefined
                .get(&device.predefined_device_id)
                .cloned()
                .unwrap_or_else(|| ("unknown".into(), 0));
            DeviceLiveness {
                id: device.id,
                device_id: device.device_id,
                predefined_device_name: name,
                device_type: types
                    .get(&type_id)
                    .cloned()
                    .unwrap_or_else(|| "unknown".into()),
                live,
                last_seen: seen_utc.map(|t| t.to_rfc3339()),
            }
        })
        .collect();
    Ok(LivenessSummary {
        total,
        live,
        devices,
    })
}

/// Agrégat des builds de l'org — borné par construction (upsert 1/device),
/// réduction en Rust plutôt qu'un GROUP BY pour rester dialect-free
/// (sqlite/PG, D5 v2).
pub async fn build_stats(db: &DatabaseConnection, org_id: i64) -> Result<BuildStats> {
    let rows = build_records::Entity::find()
        .filter(build_records::Column::OrgId.eq(org_id))
        .all(db)
        .await
        .map_err(|_| Error::InternalServerError)?;
    let total = rows.len() as u64;
    let succeeded = rows.iter().filter(|r| r.success).count() as u64;
    Ok(BuildStats {
        total,
        succeeded,
        // 0.0 si aucun build : jamais de NaN côté wasm.
        success_rate: if total == 0 {
            0.0
        } else {
            succeeded as f64 / total as f64
        },
    })
}

/// Dernières mesures de l'org depuis OpenObserve — **dégradée par
/// conception** : aucune erreur ne remonte, `available: false` suffit.
///
/// Constat e2e v0.92.1 : pas de sélecteur regex sur `__name__` (renvoie
/// vide) → les noms de métriques se découvrent via `/streams?type=metrics`
/// puis **une requête par nom** (`last_over_time(nom[1h])`), le tout sous
/// le timeout global `O2_TIMEOUT`.
pub async fn latest_measurements(
    db: &DatabaseConnection,
    client: &Client,
    org_id: i64,
) -> TelemetrySummary {
    let degraded = TelemetrySummary {
        available: false,
        latest: vec![],
    };
    // Lecture seule : une org sans données n'est pas provisionnée, on ne
    // provisionne pas depuis le chemin HTTP utilisateur.
    let Some(creds) = openobserve::provisioned_credentials(db, org_id)
        .await
        .ok()
        .flatten()
    else {
        return degraded;
    };
    let fetch = tokio::time::timeout(O2_TIMEOUT, async {
        let streams = client
            .metric_streams(&creds.o2_org, &creds.email_passcode)
            .await?;
        let mut samples = Vec::new();
        for name in streams
            .iter()
            .filter(|n| openobserve::valid_metric_name(n))
            .take(STREAMS_CAP)
        {
            // The instant query stamps samples with the evaluation time: the
            // real last-sample time comes from the SQL lookup (cached).
            let query = format!("last_over_time({name}[{LATEST_WINDOW}])");
            let (res, seen) = tokio::join!(
                client.prom_query(&creds.o2_org, &query, &creds.email_passcode),
                visualization::cached_last_seen(
                    client,
                    &creds.o2_org,
                    name,
                    LATEST_WINDOW_SECS,
                    &creds.email_passcode,
                ),
            );
            let seen = seen.unwrap_or_else(|e| {
                tracing::debug!(org_id, metric = %name, error = %e, "last seen not queried");
                Default::default()
            });
            // Une métrique injoignable n'emporte pas les autres.
            match res {
                Ok(resp) => samples.extend(resp.data.result.into_iter().map(|s| (s, seen.clone()))),
                Err(e) => {
                    tracing::debug!(org_id, metric = %name, erreur = %e, "stream non interrogé")
                }
            }
        }
        Ok::<_, String>(samples)
    })
    .await;
    let samples = match fetch {
        Ok(Ok(samples)) => samples,
        Ok(Err(e)) => {
            tracing::warn!(org_id, erreur = %e, "dashboard : O2 en échec, télémétrie dégradée");
            return degraded;
        }
        Err(_) => {
            tracing::warn!(
                org_id,
                "dashboard : chemin O2 expiré (3 s), télémétrie dégradée"
            );
            return degraded;
        }
    };

    // Séries sans métrique/device/valeur numérique : skip silencieux
    // (défensif — les valeurs sont déjà filtrées à l'ingest).
    let mut latest: Vec<LatestMeasurement> = samples
        .into_iter()
        .filter_map(|(s, seen)| {
            let metric = s.metric.get("__name__")?.clone();
            let device_id = s.metric.get("device_id")?.clone();
            let value: f64 = s.value.1.parse().ok()?;
            // Unknown last-sample time stays `None` (sorted last), never the
            // evaluation time.
            let timestamp = seen
                .get(&device_id)
                .and_then(|us| DateTime::from_timestamp_micros(*us))
                .map(|t| t.to_rfc3339());
            Some(LatestMeasurement {
                metric,
                device_id,
                value,
                timestamp,
            })
        })
        .collect();
    // RFC 3339 UTC (même convertisseur) : tri lexicographique sûr.
    latest.sort_by(|a, b| b.timestamp.cmp(&a.timestamp));
    latest.truncate(LATEST_CAP);
    TelemetrySummary {
        available: true,
        latest,
    }
}
