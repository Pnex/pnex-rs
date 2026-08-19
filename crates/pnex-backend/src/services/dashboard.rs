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
    build_records, device_registries, device_states, device_types, predefined_devices,
};
use crate::services::device_liveness;
use crate::services::openobserve::{self, client::Client};

/// Fenêtre de recherche du dernier échantillon de chaque série.
const LATEST_WINDOW: &str = "1h";

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

/// Liveness des devices de l'org : jointure registre × dernier état,
/// frais au sens du TTL de silence (`device_liveness::is_fresh`, même
/// définition que le reaper — pas le booléen `active`, possiblement périmé
/// entre deux ticks). Tri live d'abord, puis dernier signe décroissant,
/// puis **tronqué à `LIVENESS_CAP`** — `total`/`live` restent les comptes
/// complets de l'org.
pub async fn liveness(
    db: &DatabaseConnection,
    org_id: i64,
    silence_ttl_secs: i64,
) -> Result<LivenessSummary> {
    let rows = device_registries::Entity::find()
        .filter(device_registries::Column::OrgId.eq(org_id))
        .find_also_related(device_states::Entity)
        .all(db)
        .await
        .map_err(|_| Error::InternalServerError)?;

    // predefined → (nom du modèle, type) pour l'affichage.
    let predefined: HashMap<i64, (String, i64)> = predefined_devices::Entity::find()
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

    // Le tri porte sur le datetime réel (le RFC 3339 en chaîne ne trie
    // pas de manière fiable entre offsets différents).
    let mut devices: Vec<(DeviceLiveness, Option<DateTime<Utc>>)> = rows
        .into_iter()
        .map(|(device, state)| {
            let last_seen = state.as_ref().map(|s| s.last_seen_at);
            let seen_utc = last_seen.map(|t| t.with_timezone(&Utc));
            let live = seen_utc.is_some_and(|t| device_liveness::is_fresh(t, silence_ttl_secs));
            let (name, type_id) = predefined
                .get(&device.predefined_device_id)
                .cloned()
                .unwrap_or_else(|| ("unknown".into(), 0));
            (
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
                },
                seen_utc,
            )
        })
        .collect();
    devices.sort_by(|a, b| b.0.live.cmp(&a.0.live).then_with(|| b.1.cmp(&a.1)));

    // Compteurs sur l'ensemble, PUIS troncature de la liste.
    let live = devices.iter().filter(|(d, _)| d.live).count() as u64;
    let total = devices.len() as u64;
    devices.truncate(LIVENESS_CAP);
    Ok(LivenessSummary {
        total,
        live,
        devices: devices.into_iter().map(|(d, _)| d).collect(),
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
            // Une métrique injoignable n'emporte pas les autres.
            match client
                .prom_query(
                    &creds.o2_org,
                    &format!("last_over_time({name}[{LATEST_WINDOW}])"),
                    &creds.email_passcode,
                )
                .await
            {
                Ok(resp) => samples.extend(resp.data.result),
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
        .filter_map(|s| {
            let metric = s.metric.get("__name__")?.clone();
            let device_id = s.metric.get("device_id")?.clone();
            let value: f64 = s.value.1.parse().ok()?;
            let timestamp = DateTime::from_timestamp(s.value.0 as i64, 0).map(|t| t.to_rfc3339());
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
