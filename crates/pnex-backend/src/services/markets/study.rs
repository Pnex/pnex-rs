//! Event studies on stored prices (markets-research.md D532, D535): one event
//! of an instrument, or a rule tested over every filing that matches it, next
//! to the same measure on drawn dates. Reads only values already known at the
//! date of each event for the choice of events; the prices after it are what
//! is being measured.

use std::collections::HashMap;
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use chrono::NaiveDate;
use pnex_core::market_study::{
    self as ms, Bar, EventStudy, RuleResult, RuleWindow, WindowResult, WINDOWS,
};
use sea_orm::{ConnectionTrait, DatabaseBackend, DbErr, Statement};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::services::compute_limits::{Pool, Saturated};

const BENCHMARK_SETTING: &str = "markets.benchmark_ticker";
/// Instruments read by one rule test.
const RULE_INSTRUMENTS_MAX: usize = 300;
/// Drawn dates per instrument, for the baseline.
const BASELINE_PER_INSTRUMENT: usize = 20;

fn stmt(sql: &str, values: Vec<sea_orm::Value>) -> Statement {
    Statement::from_sql_and_values(DatabaseBackend::Postgres, sql, values)
}

/// The instrument's raw daily bars, oldest first. Its last value per date: a
/// source's correction of a price counts, like any later reading.
pub async fn bars<C: ConnectionTrait>(
    db: &C,
    org_id: i64,
    object: Uuid,
) -> Result<Vec<Bar>, DbErr> {
    let rows = db
        .query_all_raw(stmt(
            "SELECT DISTINCT ON (metric, period_end) metric, period_end::text AS d, value \
             FROM market_observations WHERE org_id = $1 AND object_id = $2 \
               AND metric IN ('close', 'volume', 'dividend', 'split') \
             ORDER BY metric, period_end, known_at DESC",
            vec![org_id.into(), object.into()],
        ))
        .await?;
    let mut days: HashMap<NaiveDate, Bar> = HashMap::new();
    for r in rows {
        let d: String = r.try_get("", "d")?;
        let Ok(date) = NaiveDate::parse_from_str(&d, "%Y-%m-%d") else {
            continue;
        };
        let metric: String = r.try_get("", "metric")?;
        let value: f64 = r.try_get("", "value")?;
        let bar = days.entry(date).or_insert(Bar {
            date,
            close: 0.0,
            volume: 0.0,
            dividend: 0.0,
            split: 1.0,
        });
        match metric.as_str() {
            "close" => bar.close = value,
            "volume" => bar.volume = value,
            "dividend" => bar.dividend = value,
            _ => bar.split = value,
        }
    }
    let mut out: Vec<Bar> = days.into_values().filter(|b| b.close > 0.0).collect();
    out.sort_by_key(|b| b.date);
    Ok(out)
}

/// The org's benchmark instrument.
async fn benchmark<C: ConnectionTrait>(db: &C, org_id: i64) -> Result<Option<Uuid>, DbErr> {
    let ticker = crate::services::ontology::packs::setting_value(db, org_id, BENCHMARK_SETTING)
        .await?
        .and_then(|v| v.as_str().map(|s| s.trim().to_uppercase()))
        .unwrap_or_default();
    let row = db
        .query_one_raw(stmt(
            "SELECT id FROM objects WHERE org_id = $1 AND type_key = 'instrument' \
               AND valid_to IS NULL AND properties ->> 'ticker' = $2",
            vec![org_id.into(), ticker.into()],
        ))
        .await?;
    row.map(|r| r.try_get("", "id")).transpose()
}

/// One event of one instrument; `None` without a benchmark or enough prices.
pub async fn one<C: ConnectionTrait>(
    db: &C,
    org_id: i64,
    instrument: Uuid,
    date: NaiveDate,
) -> Result<Option<EventStudy>, DbErr> {
    let Some(bench) = benchmark(db, org_id).await? else {
        return Ok(None);
    };
    let (stock, index) = (
        bars(db, org_id, instrument).await?,
        bars(db, org_id, bench).await?,
    );
    Ok(ms::event_study(&stock, &index, date))
}

/// A fictitious position's result; `None` without an instrument or prices.
pub async fn position<C: ConnectionTrait>(
    db: &C,
    org_id: i64,
    id: Uuid,
    props: &serde_json::Value,
) -> Result<Option<ms::PositionResult>, DbErr> {
    let date = |k: &str| {
        props
            .get(k)
            .and_then(serde_json::Value::as_str)
            .and_then(|s| NaiveDate::parse_from_str(s, "%Y-%m-%d").ok())
    };
    let Some(entry) = date("entry_on") else {
        return Ok(None);
    };
    let quantity = props
        .get("quantity")
        .and_then(serde_json::Value::as_f64)
        .unwrap_or(0.0);
    let short = props.get("side").and_then(serde_json::Value::as_str) == Some("short");
    let row = db
        .query_one_raw(stmt(
            "SELECT target_object_id AS i FROM resource_edges \
             WHERE org_id = $1 AND relation = 'position_on' AND source_object_id = $2 \
               AND valid_to IS NULL",
            vec![org_id.into(), id.into()],
        ))
        .await?;
    let Some(row) = row else {
        return Ok(None);
    };
    let instrument: Uuid = row.try_get("", "i")?;
    let stock = bars(db, org_id, instrument).await?;
    Ok(ms::position_result(
        &stock,
        short,
        quantity,
        entry,
        date("exit_on"),
    ))
}

/// Which filings a rule looks at.
#[derive(Debug, Clone, Default, PartialEq, Deserialize, Serialize)]
pub struct Rule {
    pub form: String,
    /// One item of the form (`2.02` = results in an 8-K); empty = all.
    pub item: String,
    pub from: Option<NaiveDate>,
    pub to: Option<NaiveDate>,
}

/// Why a rule test did not run.
#[derive(Debug)]
pub enum RuleError {
    Db(DbErr),
    Busy(Saturated),
}

impl std::fmt::Display for RuleError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Db(e) => e.fmt(f),
            Self::Busy(s) => s.fmt(f),
        }
    }
}

impl From<DbErr> for RuleError {
    fn from(e: DbErr) -> Self {
        Self::Db(e)
    }
}

impl From<RuleError> for loco_rs::Error {
    fn from(e: RuleError) -> Self {
        match e {
            RuleError::Db(e) => e.into(),
            RuleError::Busy(s) => s.into(),
        }
    }
}

/// Rule tests at once on this pod: any member can start one, and each reads
/// every price of up to 300 instruments.
fn rule_pool() -> &'static Pool {
    static POOL: OnceLock<Pool> = OnceLock::new();
    POOL.get_or_init(|| Pool::new("market-rule", 2, Duration::from_secs(10)))
}

/// The filings of a rule per instrument: an event is dated by its filing day
/// (D531), which only a filing itself can establish, so nothing here is
/// backfilled.
async fn filings_of<C: ConnectionTrait>(
    db: &C,
    org_id: i64,
    rule: &Rule,
) -> Result<HashMap<Uuid, Vec<NaiveDate>>, DbErr> {
    let rows = db
        .query_all_raw(stmt(
            "SELECT DISTINCT i.target_object_id AS instrument, f.properties ->> 'filed_on' AS d \
             FROM objects f \
             JOIN resource_edges e ON e.org_id = $1 AND e.relation = 'filed_by' \
               AND e.source_object_id = f.id AND e.valid_to IS NULL \
             JOIN resource_edges i ON i.org_id = $1 AND i.relation = 'issues' \
               AND i.source_object_id = e.target_object_id AND i.valid_to IS NULL \
             WHERE f.org_id = $1 AND f.type_key = 'filing' AND f.valid_to IS NULL \
               AND f.properties ->> 'form' = $2 \
               AND ($3 = '' OR ($3 = ANY(string_to_array(f.properties ->> 'items', ',')))) \
               AND ($4::date IS NULL OR (f.properties ->> 'filed_on')::date >= $4) \
               AND ($5::date IS NULL OR (f.properties ->> 'filed_on')::date <= $5)",
            vec![
                org_id.into(),
                rule.form.clone().into(),
                rule.item.clone().into(),
                rule.from.into(),
                rule.to.into(),
            ],
        ))
        .await?;
    let mut out: HashMap<Uuid, Vec<NaiveDate>> = HashMap::new();
    for r in rows {
        let d: String = r.try_get("", "d")?;
        if let Ok(date) = NaiveDate::parse_from_str(&d, "%Y-%m-%d") {
            out.entry(r.try_get("", "instrument")?)
                .or_default()
                .push(date);
        }
    }
    Ok(out)
}

fn window_values(studies: &[EventStudy], from: i32, to: i32) -> Vec<f64> {
    studies
        .iter()
        .flat_map(|s| s.windows.iter())
        .filter(|w: &&WindowResult| (w.from, w.to) == (from, to))
        .map(|w| w.abnormal)
        .collect()
}

/// A rule over the org's followed instruments, against drawn dates of the
/// same instruments. No parameter is tuned here (D535).
pub async fn rule<C: ConnectionTrait>(
    db: &C,
    org_id: i64,
    rule: &Rule,
) -> Result<RuleResult, RuleError> {
    let _slot = rule_pool().acquire().await.map_err(RuleError::Busy)?;
    let mut events: Vec<(Uuid, Vec<NaiveDate>)> =
        filings_of(db, org_id, rule).await?.into_iter().collect();
    // The same instruments on every call, whatever the map's order.
    events.sort_by_key(|(id, _)| *id);
    events.truncate(RULE_INSTRUMENTS_MAX);
    let filings = events.iter().map(|(_, d)| d.len()).sum();
    let index = Arc::new(match benchmark(db, org_id).await? {
        Some(b) => bars(db, org_id, b).await?,
        None => Vec::new(),
    });
    let (mut studied, mut drawn, mut instruments) = (Vec::new(), Vec::new(), 0);
    for (i, (instrument, dates)) in events.into_iter().enumerate() {
        let stock = bars(db, org_id, instrument).await?;
        if stock.is_empty() {
            continue;
        }
        instruments += 1;
        let index = index.clone();
        // Pure and long: off the async threads.
        let (a, b) = tokio::task::spawn_blocking(move || {
            let of = |d: &NaiveDate| ms::event_study(&stock, &index, *d);
            let a: Vec<_> = dates.iter().filter_map(of).collect();
            let b: Vec<_> = ms::baseline_dates(&stock, BASELINE_PER_INSTRUMENT, i as u64)
                .iter()
                .filter_map(of)
                .collect();
            (a, b)
        })
        .await
        .map_err(|e| DbErr::Custom(e.to_string()))?;
        studied.extend(a);
        drawn.extend(b);
    }
    let windows = WINDOWS
        .iter()
        .map(|&(from, to)| RuleWindow {
            from,
            to,
            events: ms::distribution(&window_values(&studied, from, to)),
            baseline: ms::distribution(&window_values(&drawn, from, to)),
        })
        .collect();
    Ok(RuleResult {
        filings,
        instruments,
        windows,
        benchmark_missing: index.is_empty(),
    })
}
