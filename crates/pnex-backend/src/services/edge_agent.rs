//! Edge agent ingestion (D95, docs/architecture/edge-agent.md).
//!
//! An edge agent is a device (`predefined_device = edge_agent`) speaking the
//! `/ws/device` tunnel. Its uplink is `DeviceMsg::Batch`: free-form points
//! drained from its disk queue — no predeclared catalogue:
//!
//! - every key is normalized and **discovered** on first sight (`agent_keys`
//!   row + mirror into `device_registries.discovered_measurements` so the
//!   existing metric pickers see it), bounded only by the distinct keys quota
//!   (`max_unique_measurements`);
//! - every value reaches the **live Valkey cache** (flows, memory, live
//!   dashboards); OpenObserve **history** only when the key's `record_o2`
//!   toggle is on or the point carries `record: true`;
//! - numbers/booleans → telemetry pipeline (`TelemetryPoint.record` gates
//!   the O2 batcher); text/objects → `pnex:lastj:v1:…` cache entry and, when
//!   recorded, the `ev_agent_events` O2 log stream;
//! - replays are deduplicated with a per-epoch `seq` high-water mark (Valkey,
//!   session memory as fallback) — at-least-once + dedup.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use loco_rs::prelude::*;
use redis::aio::ConnectionManager;
use sea_orm::sea_query::OnConflict;
use sea_orm::{ColumnTrait, EntityTrait, QueryFilter, Set};

use crate::models::_entities::{agent_keys, device_registries};
use crate::services::telemetry::{self, TelemetryPoint};
use pnex_core::BatchPoint;

/// Upper bound of points per `Batch` frame pushed to agents.
pub const AGENT_MAX_BATCH: u32 = 500;
/// Default distinct keys quota of a new agent.
pub const AGENT_DEFAULT_MAX_KEYS: i32 = 1000;
/// O2 log stream of non-numeric recorded values.
pub const AGENT_EVENT_STREAM: &str = "ev_agent_events";
/// Future skew tolerated on a point timestamp before clamping to now.
const MAX_FUTURE_SKEW_MS: i64 = 5 * 60 * 1000;
/// Key metadata refresh period (UI `record_o2` toggles apply within it).
const KEYS_REFRESH: Duration = Duration::from_secs(10);
/// Minimum interval between two `last_seen_at` writes of one key.
const LAST_SEEN_THROTTLE: Duration = Duration::from_secs(60);
/// TTL of an epoch high-water mark (longer than any sane queue backlog).
const HWM_TTL_SECS: i64 = 30 * 24 * 3600;
/// Max key length accepted from the wire (before normalization).
pub const AGENT_KEY_MAX_LEN: usize = 255;

async fn conn(ctx: &AppContext) -> Option<ConnectionManager> {
    crate::services::shared_valkey::conn(&ctx.config).await
}

/// Value kind stored per key (`agent_keys.kind`).
pub fn kind_of(value: &serde_json::Value) -> &'static str {
    match value {
        serde_json::Value::Number(_) => "number",
        serde_json::Value::Bool(_) => "bool",
        serde_json::Value::String(_) => "text",
        _ => "json",
    }
}

/// Numeric projection of a value for the metric pipeline (booleans → 1/0).
pub fn numeric_of(value: &serde_json::Value) -> Option<String> {
    match value {
        serde_json::Value::Number(n) => {
            let f: f64 = n.to_string().parse().ok()?;
            f.is_finite().then(|| n.to_string())
        }
        serde_json::Value::Bool(b) => Some(i64::from(*b).to_string()),
        _ => None,
    }
}

/// Clamps a producer timestamp: far-future stamps (skewed clock) become now.
pub fn clamp_ts_ms(ts_ms: i64, now_ms: i64) -> i64 {
    if ts_ms > now_ms + MAX_FUTURE_SKEW_MS || ts_ms <= 0 {
        now_ms
    } else {
        ts_ms
    }
}

fn hwm_key(device_registry_id: i64, epoch: &str) -> String {
    format!("pnex:agent:hwm:v1:{device_registry_id}:{epoch}")
}

#[derive(Debug, Clone)]
struct KeyState {
    record_o2: bool,
    unit: Option<String>,
    kind: String,
    last_seen_written: Instant,
}

/// Per-session agent state (lives in the `/ws/device` session loop).
pub struct AgentSession {
    device_registry_id: i64,
    keys: HashMap<String, KeyState>,
    loaded_at: Option<Instant>,
    max_keys: usize,
    /// epoch → highest processed seq (mirror of the Valkey mark).
    hwm: HashMap<String, u64>,
    /// Points dropped since session start (quota / unreadable key).
    pub dropped: u64,
}

impl AgentSession {
    pub fn new(device_registry_id: i64, max_keys: i32) -> Self {
        Self {
            device_registry_id,
            keys: HashMap::new(),
            loaded_at: None,
            max_keys: usize::try_from(max_keys.max(0)).unwrap_or(0),
            hwm: HashMap::new(),
            dropped: 0,
        }
    }

    /// Reloads key metadata + quota when stale (UI toggles, quota edits).
    async fn refresh(&mut self, db: &DatabaseConnection) {
        if self.loaded_at.is_some_and(|t| t.elapsed() < KEYS_REFRESH) {
            return;
        }
        if let Ok(rows) = agent_keys::Entity::find()
            .filter(agent_keys::Column::DeviceRegistryId.eq(self.device_registry_id))
            .all(db)
            .await
        {
            let old = std::mem::take(&mut self.keys);
            for r in rows {
                let last = old
                    .get(&r.key)
                    .map_or_else(Instant::now, |k| k.last_seen_written);
                self.keys.insert(
                    r.key,
                    KeyState {
                        record_o2: r.record_o2,
                        unit: r.unit,
                        kind: r.kind,
                        last_seen_written: last,
                    },
                );
            }
        }
        if let Ok(Some(dev)) = device_registries::Entity::find_by_id(self.device_registry_id)
            .one(db)
            .await
        {
            self.max_keys = usize::try_from(dev.max_unique_measurements.max(0)).unwrap_or(0);
        }
        self.loaded_at = Some(Instant::now());
    }

    /// Current distinct keys quota (pushed in `AgentConfig`).
    pub fn max_keys(&self) -> u32 {
        u32::try_from(self.max_keys).unwrap_or(u32::MAX)
    }
}

/// Identity of the device the batch belongs to.
pub struct AgentIdentity<'a> {
    pub org_id: i64,
    pub device_registry_id: i64,
    pub device_id: &'a str,
    pub pred_dev: &'a str,
}

/// Ingests one `Batch` — returns the `up_to_seq` to acknowledge (`None` for
/// an empty batch). Points at or below the epoch high-water mark are
/// duplicates: skipped, still acknowledged.
pub async fn ingest_batch(
    ctx: &AppContext,
    sess: &mut AgentSession,
    who: &AgentIdentity<'_>,
    epoch: &str,
    points: Vec<BatchPoint>,
) -> Option<u64> {
    let max_seq = points.iter().map(|p| p.seq).max()?;
    sess.refresh(&ctx.db).await;
    let valkey = conn(ctx).await;

    // High-water mark: session memory first, Valkey on the first batch of
    // an epoch (survives reconnects and server restarts).
    let hwm = match sess.hwm.get(epoch) {
        Some(h) => *h,
        None => {
            let stored = match valkey.clone() {
                Some(mut c) => redis::AsyncCommands::get::<_, Option<u64>>(
                    &mut c,
                    hwm_key(who.device_registry_id, epoch),
                )
                .await
                .ok()
                .flatten(),
                None => None,
            };
            stored.unwrap_or(0)
        }
    };

    let now = chrono::Utc::now();
    let now_ms = now.timestamp_millis();
    let mut new_keys: Vec<agent_keys::ActiveModel> = Vec::new();
    let mut touched: Vec<(String, Option<String>, &'static str)> = Vec::new();
    let mut json_samples: Vec<(String, pnex_core::CachedJsonSample, bool)> = Vec::new();
    let mut event_docs: Vec<serde_json::Value> = Vec::new();

    let mut sorted = points;
    sorted.sort_by_key(|p| p.seq);
    for p in sorted {
        if p.seq <= hwm {
            continue;
        }
        let raw_key = p.key.trim();
        if raw_key.is_empty() || raw_key.len() > AGENT_KEY_MAX_LEN {
            sess.dropped += 1;
            continue;
        }
        let name = pnex_core::normalize_measurement_name(raw_key);
        if name.is_empty() {
            sess.dropped += 1;
            continue;
        }
        let kind = kind_of(&p.value);
        let unit = p
            .unit
            .as_deref()
            .map(str::trim)
            .filter(|u| !u.is_empty())
            .map(|u| u.chars().take(64).collect::<String>());

        // Discovery (bounded by the distinct keys quota only).
        let record_o2 = match sess.keys.get_mut(&name) {
            Some(k) => {
                let changed = (unit.is_some() && unit != k.unit) || k.kind != kind;
                if changed || k.last_seen_written.elapsed() >= LAST_SEEN_THROTTLE {
                    if unit.is_some() {
                        k.unit.clone_from(&unit);
                    }
                    kind.clone_into(&mut k.kind);
                    k.last_seen_written = Instant::now();
                    touched.push((name.clone(), unit.clone(), kind));
                }
                k.record_o2
            }
            None => {
                if sess.keys.len() >= sess.max_keys {
                    sess.dropped += 1;
                    tracing::debug!(device = %who.device_id, key = %name, "agent keys quota reached — point dropped");
                    continue;
                }
                sess.keys.insert(
                    name.clone(),
                    KeyState {
                        record_o2: false,
                        unit: unit.clone(),
                        kind: kind.to_string(),
                        last_seen_written: Instant::now(),
                    },
                );
                new_keys.push(agent_keys::ActiveModel {
                    device_registry_id: Set(who.device_registry_id),
                    key: Set(name.clone()),
                    unit: Set(unit.clone()),
                    kind: Set(kind.to_string()),
                    record_o2: Set(false),
                    ..Default::default()
                });
                false
            }
        };
        let record = record_o2 || p.record;
        let ts_ms = clamp_ts_ms(p.ts_ms, now_ms);
        let timestamp = chrono::DateTime::from_timestamp_millis(ts_ms).unwrap_or(now);

        if let Some(numeric) = numeric_of(&p.value) {
            telemetry::sink().send(TelemetryPoint {
                org_id: who.org_id,
                device_registry_id: who.device_registry_id,
                device_id: who.device_id.to_string(),
                pred_dev: who.pred_dev.to_string(),
                metric_name: name,
                value: numeric,
                timestamp,
                ts_source: "device",
                source_type: "edge_agent",
                record,
            });
        } else {
            if record {
                event_docs.push(event_doc(who, &name, unit.as_deref(), &p.value, ts_ms));
            }
            json_samples.push((
                name,
                pnex_core::CachedJsonSample { v: p.value, ts_ms },
                record,
            ));
        }
    }

    if !new_keys.is_empty() {
        persist_new_keys(&ctx.db, who.device_registry_id, new_keys, &sess.keys).await;
    }
    if !touched.is_empty() {
        // Inline (bounded by the session) and grouped: one UPDATE per
        // (kind, unit) group instead of one detached task + one UPDATE per key.
        touch_keys(&ctx.db, who.device_registry_id, touched).await;
    }
    if let Some(c) = valkey.clone() {
        if !json_samples.is_empty() {
            let (org_id, device_id) = (who.org_id, who.device_id.to_string());
            tokio::spawn(async move {
                for (name, sample, record) in json_samples {
                    let key = pnex_core::last_json_cache_key(org_id, &device_id, &name);
                    let ttl = if record {
                        pnex_core::LAST_SAMPLE_TTL_SECS
                    } else {
                        pnex_core::AGENT_LIVE_ONLY_TTL_SECS
                    };
                    let Ok(payload) = serde_json::to_string(&sample) else {
                        continue;
                    };
                    if let Err(e) = crate::services::last_cache::set_if_newer(
                        c.clone(),
                        &key,
                        &payload,
                        sample.ts_ms,
                        ttl,
                    )
                    .await
                    {
                        tracing::warn!(error = %e, "agent json cache write failed");
                        return;
                    }
                }
            });
        }
    }
    if !event_docs.is_empty() {
        let ctx = ctx.clone();
        let org_id = who.org_id;
        tokio::spawn(async move {
            if let Err(e) = write_events(&ctx, org_id, &event_docs).await {
                tracing::warn!(error = %e, "agent events write failed");
            }
        });
    }

    let new_hwm = hwm.max(max_seq);
    sess.hwm.insert(epoch.to_string(), new_hwm);
    if new_hwm > hwm {
        if let Some(mut c) = valkey {
            let _ = redis::AsyncCommands::set_ex::<_, _, ()>(
                &mut c,
                hwm_key(who.device_registry_id, epoch),
                new_hwm,
                HWM_TTL_SECS as u64,
            )
            .await;
        }
    }
    Some(max_seq)
}

/// O2 log document of a recorded non-numeric value (stream
/// [`AGENT_EVENT_STREAM`]).
fn event_doc(
    who: &AgentIdentity<'_>,
    key: &str,
    unit: Option<&str>,
    value: &serde_json::Value,
    ts_ms: i64,
) -> serde_json::Value {
    let ev = pnex_core::events::EventInput {
        org_id: who.org_id,
        stream: AGENT_EVENT_STREAM.to_string(),
        level: pnex_core::events::EventLevel::Info,
        message: key.to_string(),
        topic: Some(who.device_id.to_string()),
        flow_id: None,
        node_id: who.device_id.to_string(),
        payload: value.clone(),
    };
    let mut doc = crate::services::events::document_of(&ev, ts_ms.saturating_mul(1000));
    doc["key"] = serde_json::Value::String(key.to_string());
    doc["device_id"] = serde_json::Value::String(who.device_id.to_string());
    if let Some(u) = unit {
        doc["unit"] = serde_json::Value::String(u.to_string());
    }
    doc
}

async fn write_events(
    ctx: &AppContext,
    org_id: i64,
    docs: &[serde_json::Value],
) -> std::result::Result<(), String> {
    use crate::services::openobserve::{ensure_org_credentials, Client, OpenobserveSettings};
    let Some(settings) = OpenobserveSettings::from_config(&ctx.config) else {
        return Ok(());
    };
    let client = Client::new(&settings);
    let creds = ensure_org_credentials(&ctx.db, &client, org_id).await?;
    client
        .ingest_json(
            &creds.o2_org,
            AGENT_EVENT_STREAM,
            docs,
            &creds.email_passcode,
        )
        .await
}

/// Inserts newly discovered keys (idempotent) and mirrors the full key set
/// into `discovered_measurements` (existing metric pickers).
async fn persist_new_keys(
    db: &DatabaseConnection,
    device_registry_id: i64,
    rows: Vec<agent_keys::ActiveModel>,
    all: &HashMap<String, KeyState>,
) {
    if let Err(e) = agent_keys::Entity::insert_many(rows)
        .on_conflict(
            OnConflict::columns([
                agent_keys::Column::DeviceRegistryId,
                agent_keys::Column::Key,
            ])
            .do_nothing()
            .to_owned(),
        )
        .exec_without_returning(db)
        .await
    {
        tracing::warn!(error = %e, "agent keys insert failed");
    }
    let map: serde_json::Map<String, serde_json::Value> = all
        .keys()
        .map(|k| (k.clone(), serde_json::Value::Bool(true)))
        .collect();
    if let Ok(Some(dev)) = device_registries::Entity::find_by_id(device_registry_id)
        .one(db)
        .await
    {
        let mut active: device_registries::ActiveModel = dev.into();
        active.discovered_measurements = Set(Some(serde_json::Value::Object(map)));
        let _ = active.update(db).await;
    }
}

async fn touch_keys(
    db: &DatabaseConnection,
    device_registry_id: i64,
    touched: Vec<(String, Option<String>, &'static str)>,
) {
    let now = chrono::Utc::now().fixed_offset();
    for ((unit, kind), keys) in group_touched(touched) {
        let mut q = agent_keys::Entity::update_many()
            .col_expr(agent_keys::Column::LastSeenAt, now.into())
            .col_expr(agent_keys::Column::Kind, kind.into());
        if let Some(u) = unit {
            q = q.col_expr(agent_keys::Column::Unit, u.into());
        }
        if let Err(e) = q
            .filter(agent_keys::Column::DeviceRegistryId.eq(device_registry_id))
            .filter(agent_keys::Column::Key.is_in(keys))
            .exec(db)
            .await
        {
            tracing::warn!(device = device_registry_id, error = %e, "agent keys touch failed");
        }
    }
}

/// Groups touched keys by the columns written with them (unit, kind): keys
/// of a group share one `UPDATE … WHERE key IN (…)`.
#[allow(clippy::type_complexity)]
fn group_touched(
    touched: Vec<(String, Option<String>, &'static str)>,
) -> Vec<((Option<String>, &'static str), Vec<String>)> {
    let mut groups: Vec<((Option<String>, &'static str), Vec<String>)> = Vec::new();
    for (key, unit, kind) in touched {
        match groups.iter_mut().find(|(g, _)| g.0 == unit && g.1 == kind) {
            Some((_, keys)) => keys.push(key),
            None => groups.push(((unit, kind), vec![key])),
        }
    }
    groups
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kinds_and_numeric_projection() {
        assert_eq!(kind_of(&serde_json::json!(1.5)), "number");
        assert_eq!(kind_of(&serde_json::json!(true)), "bool");
        assert_eq!(kind_of(&serde_json::json!("on")), "text");
        assert_eq!(kind_of(&serde_json::json!({"a": 1})), "json");
        assert_eq!(
            numeric_of(&serde_json::json!(21.5)).as_deref(),
            Some("21.5")
        );
        assert_eq!(numeric_of(&serde_json::json!(false)).as_deref(), Some("0"));
        assert_eq!(numeric_of(&serde_json::json!("21.5")), None);
    }

    #[test]
    fn touched_keys_are_grouped_by_written_columns() {
        let groups = group_touched(vec![
            ("a".into(), None, "number"),
            ("b".into(), Some("C".into()), "number"),
            ("c".into(), None, "number"),
            ("d".into(), None, "text"),
            ("e".into(), Some("C".into()), "number"),
        ]);
        assert_eq!(groups.len(), 3);
        assert_eq!(
            groups[0],
            ((None, "number"), vec!["a".to_string(), "c".to_string()])
        );
        assert_eq!(
            groups[1],
            (
                (Some("C".into()), "number"),
                vec!["b".to_string(), "e".to_string()]
            )
        );
        assert_eq!(groups[2], ((None, "text"), vec!["d".to_string()]));
    }

    #[test]
    fn future_timestamps_are_clamped() {
        let now = 1_800_000_000_000;
        assert_eq!(clamp_ts_ms(now - 3_600_000, now), now - 3_600_000);
        assert_eq!(clamp_ts_ms(now + 60_000, now), now + 60_000);
        assert_eq!(clamp_ts_ms(now + 3_600_000, now), now);
        assert_eq!(clamp_ts_ms(0, now), now);
    }
}
