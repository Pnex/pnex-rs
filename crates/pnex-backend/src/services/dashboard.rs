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

/// Requête instantanée catch-all : dernier échantillon de chaque série de
/// l'org sur 1 h. L'org O2 est dédiée à l'org PNEX (D2), inutile
/// d'énumérer les noms de métriques — couvre les devices dynamiques non
/// encore découverts. Plan B si O2 rejette le sélecteur regex :
/// énumérer les noms connus (constante isolée pour ça).
const LATEST_QUERY: &str = r#"last_over_time({__name__=~".+"}[1h])"#;

/// Timeout du seul appel O2 du summary — le dashboard répond quoi qu'il
/// arrive, une page ne doit pas traîner 10 s (timeout http du client)
/// parce qu'O2 est lent.
const O2_TIMEOUT: Duration = Duration::from_secs(3);

/// Borné côté serveur : la table du dashboard reste lisible.
const LATEST_CAP: usize = 12;

/// Liveness des devices de l'org : jointure registre × dernier état,
/// frais au sens du TTL de silence (`device_liveness::is_fresh`, même
/// définition que le reaper — pas le booléen `active`, potentially périmé
/// entre deux ticks). Tri live d'abord, puis dernier signe décroissant.
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
            let live = seen_utc
                .is_some_and(|t| device_liveness::is_fresh(t, silence_ttl_secs));
            let (name, type_id) = predefined
                .get(&device.predefined_device_id)
                .cloned()
                .unwrap_or_else(|| ("unknown".into(), 0));
            (
                DeviceLiveness {
                    id: device.id,
                    device_id: device.device_id,
                    predefined_device_name: name,
                    device_type: types.get(&type_id).cloned().unwrap_or_else(|| "unknown".into()),
                    live,
                    last_seen: seen_utc.map(|t| t.to_rfc3339()),
                },
                seen_utc,
            )
        })
        .collect();
    devices.sort_by(|a, b| b.0.live.cmp(&a.0.live).then_with(|| b.1.cmp(&a.1)));

    let live = devices.iter().filter(|(d, _)| d.live).count() as u64;
    Ok(LivenessSummary {
        total: devices.len() as u64,
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
    let query = tokio::time::timeout(
        O2_TIMEOUT,
        client.prom_query(&creds.o2_org, LATEST_QUERY, &creds.email_passcode),
    )
    .await;
    let samples = match query {
        Ok(Ok(resp)) => resp.data.result,
        Ok(Err(e)) => {
            tracing::warn!(org_id, erreur = %e, "dashboard : query O2 en échec, télémétrie dégradée");
            return degraded;
        }
        Err(_) => {
            tracing::warn!(org_id, "dashboard : query O2 expirée (3 s), télémétrie dégradée");
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
            let timestamp = DateTime::from_timestamp(s.value.0 as i64, 0)
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
