//! Dashboard (2026-08-19) — read-only for
//! `GET /api/v1/dashboard/summary`: PG liveness, PG build stats,
//! latest OpenObserve measurements.
//!
//! Doctrine: the telemetry branch **never** fails the request
//! (unprovisioned org, O2 unreachable, 3 s timeout →
//! `telemetry.available == false`) and **never** triggers
//! provisioning (`provisioned_credentials`, read-only). The PG
//! sections are always served.

use std::collections::HashMap;
use std::time::Duration;

use chrono::{DateTime, Utc};
use loco_rs::prelude::*;
use pnex_core::{BuildStats, DeviceLiveness, LatestMeasurement, LivenessSummary, TelemetrySummary};
use sea_orm::{ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter};

use crate::models::_entities::{
    build_records, device_registries, device_types, flows, predefined_devices,
};
use crate::services::device_liveness;
use crate::services::openobserve::{self, client::Client};
use crate::services::visualization;

/// Search window for the latest sample of each series.
const LATEST_WINDOW: &str = "1h";
/// [`LATEST_WINDOW`] in seconds (SQL last-sample lookup).
const LATEST_WINDOW_SECS: i64 = 3_600;

/// Timeout of the WHOLE O2 path of the summary (stream discovery +
/// queries) — the dashboard answers no matter what, a page must not
/// hang for 10 s (client http timeout) because O2 is slow.
const O2_TIMEOUT: Duration = Duration::from_secs(3);

/// Capped server-side: the dashboard table stays readable (user request
/// 2026-08-19 — « only latest ~10 », not the whole history).
const LATEST_CAP: usize = 10;

/// Same for the liveness list: the ~10 most recently active devices
/// (live first) — the card counters are still computed over all of
/// the org's devices.
const LIVENESS_CAP: usize = 10;

/// Max number of streams queried (defensive: bounded selector even if
/// an org has dozens of dynamic metrics).
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

    // Model of every device of the org: per-type counts cover the whole org,
    // the display rows reuse the same lookup.
    let all_pd: Vec<i64> = device_registries::Entity::find()
        .select_only()
        .column(device_registries::Column::PredefinedDeviceId)
        .filter(device_registries::Column::OrgId.eq(org_id))
        .into_tuple::<i64>()
        .all(db)
        .await
        .map_err(|_| Error::InternalServerError)?;
    let mut pd_ids = all_pd.clone();
    pd_ids.sort_unstable();
    pd_ids.dedup();
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
    let mut by_type = std::collections::BTreeMap::new();
    for pd in &all_pd {
        let name = predefined
            .get(pd)
            .and_then(|(_, type_id)| types.get(type_id).cloned())
            .unwrap_or_else(|| "unknown".into());
        *by_type.entry(name).or_insert(0u64) += 1;
    }
    Ok(LivenessSummary {
        total,
        live,
        devices,
        by_type,
    })
}

/// Build aggregate of the org, bounded by construction (one upsert per
/// device), reduced in Rust.
pub async fn build_stats(db: &DatabaseConnection, org_id: i64) -> Result<BuildStats> {
    let rows = build_records::Entity::find()
        .filter(build_records::Column::OrgId.eq(org_id))
        .all(db)
        .await
        .map_err(|_| Error::InternalServerError)?;
    let total = rows.len() as u64;
    let succeeded = rows
        .iter()
        .filter(|r| r.build_phase == crate::services::firmware::PHASE_SUCCEEDED)
        .count() as u64;
    Ok(BuildStats {
        total,
        succeeded,
        // 0.0 if no build: never NaN on the wasm side.
        success_rate: if total == 0 {
            0.0
        } else {
            succeeded as f64 / total as f64
        },
    })
}

/// The org's latest measurements from OpenObserve — **degraded by
/// design**: no error bubbles up, `available: false` is enough.
///
/// e2e finding v0.92.1: no regex selector on `__name__` (returns
/// empty) → metric names are discovered via `/streams?type=metrics`
/// then **one query per name** (`last_over_time(nom[1h])`), all under
/// the global `O2_TIMEOUT`.
pub async fn latest_measurements(
    db: &DatabaseConnection,
    client: &Client,
    org_id: i64,
) -> TelemetrySummary {
    let degraded = TelemetrySummary {
        available: false,
        latest: vec![],
    };
    // Read-only: an org without data is not provisioned, we do not
    // provision from the user HTTP path.
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
            // One unreachable metric does not take the others down.
            match res {
                Ok(resp) => samples.extend(resp.data.result.into_iter().map(|s| (s, seen.clone()))),
                Err(e) => {
                    tracing::debug!(org_id, metric = %name, erreur = %e, "stream not queried")
                }
            }
        }
        Ok::<_, String>(samples)
    })
    .await;
    let samples = match fetch {
        Ok(Ok(samples)) => samples,
        Ok(Err(e)) => {
            tracing::warn!(org_id, erreur = %e, "dashboard: O2 failed, telemetry degraded");
            return degraded;
        }
        Err(_) => {
            tracing::warn!(
                org_id,
                "dashboard: O2 path timed out (3 s), telemetry degraded"
            );
            return degraded;
        }
    };

    // Series without metric/device/numeric value: silent skip
    // (defensive — values are already filtered at ingest).
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
    // Series of deleted flows (`flow_<id>` virtual devices) are dropped (O59);
    // on a database error they stay, the summary never fails.
    let flow_ids: Vec<i64> = latest
        .iter()
        .filter_map(|m| flow_series_id(&m.device_id))
        .collect();
    if !flow_ids.is_empty() {
        use sea_orm::QuerySelect;
        let live = flows::Entity::find()
            .select_only()
            .column(flows::Column::Id)
            .filter(flows::Column::OrgId.eq(org_id))
            .filter(flows::Column::Id.is_in(flow_ids))
            .into_tuple::<i64>()
            .all(db)
            .await;
        match live {
            Ok(live) => {
                latest.retain(|m| flow_series_id(&m.device_id).is_none_or(|id| live.contains(&id)))
            }
            Err(e) => tracing::warn!(org_id, error = %e, "flow series not filtered"),
        }
    }
    // RFC 3339 UTC (same converter): lexicographic sort is safe.
    latest.sort_by(|a, b| b.timestamp.cmp(&a.timestamp));
    latest.truncate(LATEST_CAP);
    TelemetrySummary {
        available: true,
        latest,
    }
}

/// Flow id of a flow's virtual device series (`flow_<id>`, see
/// `pnex_core::flow::config`), `None` for a real device.
fn flow_series_id(device_id: &str) -> Option<i64> {
    device_id.strip_prefix("flow_")?.parse().ok()
}

#[cfg(test)]
mod tests {
    use super::flow_series_id;

    #[test]
    fn only_flow_virtual_devices_have_a_flow_id() {
        assert_eq!(flow_series_id("flow_58"), Some(58));
        assert_eq!(flow_series_id("flow_x"), None);
        assert_eq!(flow_series_id("esp32-a1"), None);
        assert_eq!(flow_series_id("12"), None);
    }
}
