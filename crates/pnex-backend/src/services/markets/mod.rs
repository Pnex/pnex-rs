//! Dated values read from primary sources (markets-research.md D541). A
//! value is never overwritten: a restatement adds a row, and a reading "as
//! known at t" takes, for each period, the last row known by then.

pub mod accounts;
pub mod chains;
pub mod chart;
pub mod decisions;
pub mod history;
pub mod moves;
pub mod prediction;
pub mod study;
pub mod summary;
pub mod world;

use chrono::{DateTime, FixedOffset};
use pnex_core::markets::{ObservationTarget, PlannedObservation};
use pnex_core::ontology::api::ObservationView;
use sea_orm::{ConnectionTrait, DatabaseBackend, DbErr, Statement};
use uuid::Uuid;

use crate::services::secrets::Keyring;

const SWEEP_EVERY: std::time::Duration = std::time::Duration::from_secs(300);

/// The passes of the markets still outside the pipelines: the MT5 inbox
/// and the prediction markets (pipeline-engine.md PE6, PE8).
pub fn spawn_scheduler(ctx: &loco_rs::app::AppContext) {
    init(ctx);
    let ctx = ctx.clone();
    tokio::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_secs(180)).await;
        loop {
            if !crate::services::ontology::imports::stop::stopping()
                && crate::services::singleton::my_turn(&ctx.db, "market-documents", SWEEP_EVERY)
                    .await
            {
                let today = chrono::Utc::now().date_naive();
                if let Err(e) = history::sweep(&ctx, today).await {
                    tracing::warn!(code = e.code(), "price history sweep failed");
                }
                let hosts = prediction::Hosts::default();
                if let Err(e) = prediction::sweep(&ctx, &hosts, chrono::Utc::now()).await {
                    tracing::warn!("prediction markets sweep failed: {e}");
                }
            }
            tokio::time::sleep(SWEEP_EVERY).await;
        }
    });
}

static RING: std::sync::OnceLock<Keyring> = std::sync::OnceLock::new();
/// Hosts of mocks of keyed sources: integration tests only, never set by the app.
static TEST_HOSTS: std::sync::Mutex<Vec<String>> = std::sync::Mutex::new(Vec::new());

/// The vault keyring of the imports, which run without an `AppContext`.
pub fn init(ctx: &loco_rs::app::AppContext) {
    if let Ok(ring) = Keyring::from_config(&ctx.config) {
        let _ = RING.set(ring);
    }
}

pub fn keyring() -> Option<&'static Keyring> {
    RING.get()
}

pub fn allow_test_host(base: &str) {
    if let Ok(mut hosts) = TEST_HOSTS.lock() {
        hosts.push(base.to_string());
    }
}

/// The mocks allowed, for the sweeps that pick their URLs in SQL.
pub fn test_hosts() -> Vec<String> {
    TEST_HOSTS.lock().map(|h| h.clone()).unwrap_or_default()
}

pub fn test_host_allowed(url: &str) -> bool {
    TEST_HOSTS
        .lock()
        .is_ok_and(|hosts| hosts.iter().any(|b| url.starts_with(b.as_str())))
}

/// Rows written per statement.
const BATCH: usize = 2000;
/// Most values one object's reading returns.
const READ_MAX: i64 = 20_000;

fn stmt(sql: &str, values: Vec<sea_orm::Value>) -> Statement {
    Statement::from_sql_and_values(DatabaseBackend::Postgres, sql, values)
}

/// Stores a run's values and says how many were written: what the object
/// already holds at that date unchanged is skipped. A value without a
/// publication day counts as known now, and the first ones of a metric are
/// flagged `backfilled`: their real publication date is lost, a past-dated
/// test must not read them. A filing
/// counts as known at the end of its day, never before the market could
/// have read it.
pub async fn store<C: ConnectionTrait>(
    db: &C,
    org_id: i64,
    target: &ObservationTarget,
    rows: &[PlannedObservation],
    source_ref: &str,
) -> Result<usize, DbErr> {
    let mut stored = 0;
    // One instant for the whole run: a later batch must still see the metric
    // as new, or only the first batch of a long history would be flagged.
    let run_at = chrono::Utc::now().fixed_offset();
    for chunk in rows.chunks(BATCH) {
        let col = |f: fn(&PlannedObservation) -> String| -> sea_orm::Value {
            chunk.iter().map(f).collect::<Vec<String>>().into()
        };
        stored += db
            .execute_raw(stmt(
                "WITH input AS ( \
               SELECT t.key, t.metric, t.unit, t.p_start::date AS p_start, t.p_end::date AS p_end, \
                 t.value, NULLIF(t.source, '') AS source, \
                 (NULLIF(t.known, '')::date + 1)::timestamp AT TIME ZONE 'UTC' \
                   - interval '1 second' AS published \
               FROM unnest($4::text[], $5::text[], $6::text[], $7::text[], $8::text[], \
                            $9::text[], $10::float8[], $11::text[]) \
                 AS t(key, metric, unit, p_start, p_end, known, value, source) \
             ) \
             INSERT INTO market_observations \
               (org_id, object_id, metric, unit, period_start, period_end, known_at, value, \
                source_ref, backfilled) \
             SELECT $1, o.id, i.metric, i.unit, i.p_start, i.p_end, \
               COALESCE(i.published, $13), i.value, COALESCE(i.source, $12), \
               i.published IS NULL AND NOT EXISTS (SELECT 1 FROM market_observations f \
                 WHERE f.org_id = $1 AND f.object_id = o.id AND f.metric = i.metric \
                   AND f.known_at < $13) \
             FROM input i \
             JOIN objects o ON o.org_id = $1 AND o.type_key = $2 AND o.valid_to IS NULL \
               AND o.properties ->> $3 = i.key \
             LEFT JOIN LATERAL ( \
               SELECT x.value FROM market_observations x \
               WHERE x.org_id = $1 AND x.object_id = o.id AND x.metric = i.metric \
                 AND x.unit = i.unit AND x.period_start = i.p_start AND x.period_end = i.p_end \
                 AND x.known_at <= COALESCE(i.published, $13) \
               ORDER BY x.known_at DESC LIMIT 1 \
             ) latest ON true \
             WHERE latest.value IS DISTINCT FROM i.value \
             ON CONFLICT DO NOTHING",
                vec![
                    org_id.into(),
                    target.type_key.clone().into(),
                    target.property.clone().into(),
                    col(|o| o.key.clone()),
                    col(|o| o.metric.clone()),
                    col(|o| o.unit.clone()),
                    col(|o| o.period_start.clone()),
                    col(|o| o.period_end.clone()),
                    col(|o| o.known_on.clone().unwrap_or_default()),
                    chunk.iter().map(|o| o.value).collect::<Vec<f64>>().into(),
                    col(|o| o.source.clone().unwrap_or_default()),
                    source_ref.into(),
                    run_at.into(),
                ],
            ))
            .await?
            .rows_affected() as usize;
    }
    Ok(stored)
}

/// The values of an object, latest period first: for each period the last
/// value known at `as_of` (default: now). With `as_of`, the values whose
/// publication date is lost stay out unless asked (D531).
pub async fn read<C: ConnectionTrait>(
    db: &C,
    org_id: i64,
    object: Uuid,
    metric: Option<&str>,
    as_of: Option<DateTime<FixedOffset>>,
    include_backfilled: Option<bool>,
) -> Result<Vec<ObservationView>, DbErr> {
    let backfilled = include_backfilled.unwrap_or(as_of.is_none());
    let rows = db
        .query_all_raw(stmt(
            "SELECT * FROM ( \
               SELECT DISTINCT ON (metric, unit, period_start, period_end) \
                 metric, unit, period_start::text AS period_start, period_end::text AS period_end, \
                 to_char(known_at AT TIME ZONE 'UTC', 'YYYY-MM-DD\"T\"HH24:MI:SS\"Z\"') AS known_at, \
                 value, source_ref, backfilled \
               FROM market_observations \
               WHERE org_id = $1 AND object_id = $2 AND ($3::text IS NULL OR metric = $3) \
                 AND ($4::timestamptz IS NULL OR known_at <= $4) AND ($5 OR NOT backfilled) \
               ORDER BY metric, unit, period_start, period_end, known_at DESC \
             ) v ORDER BY period_end DESC, metric, unit, period_start DESC LIMIT $6",
            vec![
                org_id.into(),
                object.into(),
                metric.map(str::to_string).into(),
                as_of.into(),
                backfilled.into(),
                READ_MAX.into(),
            ],
        ))
        .await?;
    rows.iter()
        .map(|r| {
            Ok(ObservationView {
                metric: r.try_get("", "metric")?,
                unit: r.try_get("", "unit")?,
                period_start: r.try_get("", "period_start")?,
                period_end: r.try_get("", "period_end")?,
                known_at: r.try_get("", "known_at")?,
                value: r.try_get("", "value")?,
                source_ref: r.try_get("", "source_ref")?,
                backfilled: r.try_get("", "backfilled")?,
            })
        })
        .collect()
}
