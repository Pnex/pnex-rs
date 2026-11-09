//! Decisions of an external model (markets-research.md appendix G): bars
//! read at a date from the price tables (D892), decisions posted by a
//! service account (D894) and their scores (D896). Advice only (D895):
//! nothing here reaches a terminal or places an order.

use arrow_array::cast::AsArray;
use arrow_array::types::{Decimal128Type, TimestampMicrosecondType};
use chrono::{DateTime, FixedOffset, SecondsFormat, Utc};
use futures_util::{StreamExt, TryStreamExt};
use pnex_core::markets::decisions::{
    self as dec, Bar, BarOut, Decision, ModelScores, NewDecision, OPTIONS,
};
use sea_orm::{ConnectionTrait, DatabaseConnection, DbErr, QueryResult};
use uuid::Uuid;

use super::stmt;
use crate::services::artifact_store::S3Store;
use crate::services::datasets::{self, parquet, DatasetError};

/// Files one read may open: past it, the period must be narrowed.
// shortcut: a live MT5 table writes ~288 files a day, so a long window opens
// many; upgrade = the monthly compaction planned by D808 if real loads show it.
pub const FILES_MAX: usize = 20_000;
const READ_CONCURRENCY: usize = 8;
const COLUMNS: [&str; 8] = [
    "instrument",
    "ts",
    "open",
    "high",
    "low",
    "close",
    "volume",
    "source",
];
const DAY_US: i64 = 86_400_000_000;

#[derive(Debug)]
pub enum BarsError {
    Dataset(DatasetError),
    TooManyFiles,
}

impl From<DatasetError> for BarsError {
    fn from(e: DatasetError) -> Self {
        Self::Dataset(e)
    }
}

impl From<DbErr> for BarsError {
    fn from(e: DbErr) -> Self {
        Self::Dataset(DatasetError::Db(e))
    }
}

/// The org's table of a `source`, `None` when never written.
pub async fn table_of<C: ConnectionTrait>(
    db: &C,
    org: i64,
    source: &str,
) -> Result<Option<Uuid>, DbErr> {
    let Some(name) = dec::bar_table(source) else {
        return Ok(None);
    };
    db.query_one_raw(stmt(
        "SELECT id FROM datasets WHERE org_id = $1 AND name = $2",
        vec![org.into(), name.into()],
    ))
    .await?
    .map(|r| r.try_get::<Uuid>("", "id"))
    .transpose()
}

/// What a read asks for, times in µs since the epoch (UTC).
pub struct BarsQuery<'a> {
    pub instrument: &'a str,
    pub source: &'a str,
    pub interval_us: i64,
    pub from_us: Option<i64>,
    pub to_us: Option<i64>,
    pub limit: usize,
    pub as_of_us: i64,
}

/// Bars of `q` closed at `as_of`: the first `limit` from `from`, or the last
/// `limit` before `to` without `from`.
pub async fn bars(
    db: &DatabaseConnection,
    store: &S3Store,
    org: i64,
    table: Uuid,
    q: &BarsQuery<'_>,
) -> Result<Vec<Bar>, BarsError> {
    let to = q.to_us.unwrap_or(i64::MAX).min(q.as_of_us);
    let files = datasets::view_files(db, org, table, None).await?;
    if let Some(from) = q.from_us {
        let mut out = window(store, &files, q, from, to).await?;
        out.truncate(q.limit);
        return Ok(out);
    }
    // The last `limit` bars: a window widened until it holds them (weekends
    // and gaps make the span unknown).
    let mut span = q.limit as i64 * q.interval_us * 3 / 2 + 4 * DAY_US;
    loop {
        let mut out = window(store, &files, q, to.saturating_sub(span), to).await?;
        if out.len() >= q.limit || span > 40 * 365 * DAY_US {
            let cut = out.len().saturating_sub(q.limit);
            return Ok(out.split_off(cut));
        }
        span *= 4;
    }
}

/// Bars opening in [`from`, `to`), from the files that may hold them.
async fn window(
    store: &S3Store,
    files: &[datasets::ViewFile],
    q: &BarsQuery<'_>,
    from: i64,
    to: i64,
) -> Result<Vec<Bar>, BarsError> {
    // A row is never newer than its commit: older files cannot reach `from`.
    let jobs: Vec<(String, u64)> = files
        .iter()
        .filter(|f| f.committed_at.is_none_or(|c| c.timestamp_micros() >= from))
        .map(|f| (f.key.clone(), f.bytes))
        .collect();
    if jobs.len() > FILES_MAX {
        return Err(BarsError::TooManyFiles);
    }
    let read_to = to.saturating_add(q.interval_us);
    let batches: Vec<Vec<arrow_array::RecordBatch>> = futures_util::stream::iter(jobs)
        .map(|(key, bytes)| {
            let store = store.clone();
            async move {
                parquet::read_time_range(&store, &key, bytes, &COLUMNS, "ts", from, read_to).await
            }
        })
        .buffered(READ_CONCURRENCY)
        .try_collect()
        .await
        .map_err(|e| BarsError::Dataset(DatasetError::Storage(e.to_string())))?;
    let mut minutes = Vec::new();
    for batch in batches.iter().flatten() {
        let (Some(inst), Some(src), Some(ts)) = (
            batch
                .column_by_name("instrument")
                .and_then(|c| c.as_string_opt::<i32>()),
            batch
                .column_by_name("source")
                .and_then(|c| c.as_string_opt::<i32>()),
            batch
                .column_by_name("ts")
                .and_then(|c| c.as_primitive_opt::<TimestampMicrosecondType>()),
        ) else {
            continue;
        };
        let dec = |name: &str| {
            batch
                .column_by_name(name)
                .and_then(|c| c.as_primitive_opt::<Decimal128Type>())
        };
        let (Some(o), Some(h), Some(l), Some(c), Some(v)) = (
            dec("open"),
            dec("high"),
            dec("low"),
            dec("close"),
            dec("volume"),
        ) else {
            continue;
        };
        for i in 0..batch.num_rows() {
            if inst.value(i) != q.instrument || src.value(i) != q.source {
                continue;
            }
            minutes.push(Bar {
                ts_us: ts.value(i),
                open: o.value(i),
                high: h.value(i),
                low: l.value(i),
                close: c.value(i),
                volume: v.value(i),
            });
        }
    }
    Ok(dec::resample(minutes, q.interval_us, from, to, q.as_of_us))
}

fn rfc3339_us(us: i64) -> String {
    DateTime::<Utc>::from_timestamp_micros(us)
        .unwrap_or_default()
        .to_rfc3339_opts(SecondsFormat::Secs, true)
}

pub fn bar_out(b: &Bar) -> BarOut {
    BarOut {
        ts: rfc3339_us(b.ts_us),
        open: dec::decimal_text(b.open),
        high: dec::decimal_text(b.high),
        low: dec::decimal_text(b.low),
        close: dec::decimal_text(b.close),
        volume: dec::decimal_text(b.volume),
    }
}

/// Whether the org has prices of this instrument: a loaded partition or an
/// MT5 symbol announced by one of its terminals.
pub async fn known_instrument<C: ConnectionTrait>(
    db: &C,
    org: i64,
    instrument: &str,
) -> Result<bool, DbErr> {
    Ok(db
        .query_one_raw(stmt(
            "SELECT EXISTS (SELECT 1 FROM market_history_partitions \
                 WHERE org_id = $1 AND instrument = $2 AND state = 'done') \
              OR EXISTS (SELECT 1 FROM mt5_symbols s JOIN mt5_terminals t ON t.id = s.terminal_id \
                 WHERE t.org_id = $1 AND s.symbol = $2) AS known",
            vec![org.into(), instrument.into()],
        ))
        .await?
        .map(|r| r.try_get::<bool>("", "known"))
        .transpose()?
        .unwrap_or(false))
}

#[derive(Debug)]
pub enum CreateError {
    Conflict,
    Quota,
    Db(DbErr),
}

impl From<DbErr> for CreateError {
    fn from(e: DbErr) -> Self {
        Self::Db(e)
    }
}

const COLS: &str = "id, model, model_version, instrument, source, interval, horizon_bars, \
     as_of, probabilities, best, confidence, barriers, inputs, created_at";
const SELECT: &str =
    "SELECT id, model, model_version, instrument, source, interval, horizon_bars, \
     as_of, probabilities, best, confidence, barriers, inputs, created_at FROM market_decisions";

fn decision(r: &QueryResult) -> Result<Decision, DbErr> {
    let at = |k: &str| -> Result<String, DbErr> {
        Ok(r.try_get::<DateTime<FixedOffset>>("", k)?
            .with_timezone(&Utc)
            .to_rfc3339_opts(SecondsFormat::Micros, true))
    };
    let barriers: Option<serde_json::Value> = r.try_get("", "barriers")?;
    Ok(Decision {
        id: r.try_get::<Uuid>("", "id")?.to_string(),
        model: r.try_get("", "model")?,
        model_version: r.try_get("", "model_version")?,
        instrument: r.try_get("", "instrument")?,
        source: r.try_get("", "source")?,
        interval: r.try_get("", "interval")?,
        horizon_bars: r.try_get::<i32>("", "horizon_bars")?.max(0) as u32,
        as_of: at("as_of")?,
        options: OPTIONS.iter().map(|o| o.to_string()).collect(),
        probabilities: r.try_get("", "probabilities")?,
        best: r.try_get("", "best")?,
        confidence: r.try_get("", "confidence")?,
        barriers: barriers.and_then(|b| serde_json::from_value(b).ok()),
        inputs: r.try_get("", "inputs")?,
        created_at: at("created_at")?,
    })
}

/// Stores a checked decision; `Ok((false, d))` when the very same one was
/// already there (idempotent replay).
pub async fn create(
    db: &DatabaseConnection,
    org: i64,
    service_account: Option<Uuid>,
    d: &NewDecision,
    as_of: DateTime<Utc>,
) -> Result<(bool, Decision), CreateError> {
    let today: i64 = db
        .query_one_raw(stmt(
            "SELECT count(*) AS n FROM market_decisions \
             WHERE org_id = $1 AND created_at >= date_trunc('day', now() AT TIME ZONE 'UTC') AT TIME ZONE 'UTC'",
            vec![org.into()],
        ))
        .await?
        .map(|r| r.try_get("", "n"))
        .transpose()?
        .unwrap_or(0);
    let (best, confidence) = dec::best_and_confidence(&d.probabilities);
    let barriers = d
        .barriers
        .map(|b| serde_json::to_value(b).unwrap_or_default());
    let key = vec![
        org.into(),
        d.model.clone().into(),
        d.model_version.clone().into(),
        d.instrument.clone().into(),
        d.interval.clone().into(),
        as_of.into(),
    ];
    let existing = || async {
        db.query_one_raw(stmt(
            &format!(
                "{SELECT} WHERE org_id = $1 AND model = $2 AND model_version = $3 \
                 AND instrument = $4 AND interval = $5 AND as_of = $6"
            ),
            key.clone(),
        ))
        .await?
        .as_ref()
        .map(decision)
        .transpose()
    };
    let same = |e: &Decision| {
        e.source == d.source
            && e.horizon_bars == d.horizon_bars
            && e.probabilities == d.probabilities
            && e.barriers == d.barriers
            && e.inputs == d.inputs
    };
    if today >= dec::DAILY_MAX {
        // A replay of a stored decision is still answered.
        return match existing().await? {
            Some(e) if same(&e) => Ok((false, e)),
            _ => Err(CreateError::Quota),
        };
    }
    let mut values = key.clone();
    values.extend([
        Uuid::new_v4().into(),
        d.source.clone().into(),
        (d.horizon_bars as i32).into(),
        d.probabilities.clone().into(),
        OPTIONS[best].into(),
        confidence.into(),
        barriers.into(),
        d.inputs.clone().into(),
        service_account.into(),
    ]);
    let inserted = db
        .query_one_raw(stmt(
            &format!(
                "INSERT INTO market_decisions (org_id, model, model_version, instrument, interval, as_of, \
                   id, source, horizon_bars, probabilities, best, confidence, barriers, inputs, service_account_id) \
                 VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15) \
                 ON CONFLICT (org_id, model, model_version, instrument, interval, as_of) DO NOTHING \
                 RETURNING {COLS}"
            ),
            values,
        ))
        .await?;
    if let Some(r) = inserted {
        return Ok((true, decision(&r)?));
    }
    match existing().await? {
        Some(e) if same(&e) => Ok((false, e)),
        _ => Err(CreateError::Conflict),
    }
}

/// Filters of a list or of the scores.
#[derive(Default)]
pub struct Filter<'a> {
    pub model: Option<&'a str>,
    pub model_version: Option<&'a str>,
    pub instrument: Option<&'a str>,
}

fn where_of(org: i64, f: &Filter<'_>) -> (String, Vec<sea_orm::Value>) {
    let mut sql = String::from(" WHERE org_id = $1");
    let mut values: Vec<sea_orm::Value> = vec![org.into()];
    for (col, v) in [
        ("model", f.model),
        ("model_version", f.model_version),
        ("instrument", f.instrument),
    ] {
        if let Some(v) = v {
            values.push(v.into());
            sql.push_str(&format!(" AND {col} = ${}", values.len()));
        }
    }
    (sql, values)
}

/// Newest first.
pub async fn list<C: ConnectionTrait>(
    db: &C,
    org: i64,
    f: &Filter<'_>,
    limit: u64,
    offset: u64,
) -> Result<(u64, Vec<Decision>), DbErr> {
    let (w, values) = where_of(org, f);
    let count: i64 = db
        .query_one_raw(stmt(
            &format!("SELECT count(*) AS n FROM market_decisions{w}"),
            values.clone(),
        ))
        .await?
        .map(|r| r.try_get("", "n"))
        .transpose()?
        .unwrap_or(0);
    let rows = db
        .query_all_raw(stmt(
            &format!(
                "{SELECT}{w} ORDER BY as_of DESC, model, instrument LIMIT {limit} OFFSET {offset}"
            ),
            values,
        ))
        .await?;
    let items = rows.iter().map(decision).collect::<Result<Vec<_>, _>>()?;
    Ok((count.max(0) as u64, items))
}

/// Decisions measured per group, the newest of each.
// shortcut: scores read the last SCORED_MAX decisions of a group; upgrade =
// outcomes stored once their horizon passed, when a model runs for years.
pub const SCORED_MAX: u64 = 5_000;

/// (model, version, instrument, source, interval) groups of the filter.
pub async fn groups<C: ConnectionTrait>(
    db: &C,
    org: i64,
    f: &Filter<'_>,
) -> Result<Vec<(String, String, String, String, String)>, DbErr> {
    let (w, values) = where_of(org, f);
    let rows = db
        .query_all_raw(stmt(
            &format!(
                "SELECT DISTINCT model, model_version, instrument, source, interval FROM market_decisions{w} \
                 ORDER BY model, model_version, instrument, source, interval LIMIT 100"
            ),
            values,
        ))
        .await?;
    rows.iter()
        .map(|r| {
            Ok((
                r.try_get("", "model")?,
                r.try_get("", "model_version")?,
                r.try_get("", "instrument")?,
                r.try_get("", "source")?,
                r.try_get("", "interval")?,
            ))
        })
        .collect()
}

/// Scores of one group, measured on the bars of its own source (D896).
pub async fn score_group(
    db: &DatabaseConnection,
    store: &S3Store,
    org: i64,
    table: Option<Uuid>,
    g: &(String, String, String, String, String),
    threshold: f64,
) -> Result<ModelScores, BarsError> {
    let (model, version, instrument, source, interval) = g;
    let rows = db
        .query_all_raw(stmt(
            &format!(
                "{SELECT} WHERE org_id = $1 AND model = $2 AND model_version = $3 AND instrument = $4 \
                 AND source = $5 AND interval = $6 ORDER BY as_of DESC LIMIT {SCORED_MAX}"
            ),
            vec![
                org.into(),
                model.clone().into(),
                version.clone().into(),
                instrument.clone().into(),
                source.clone().into(),
                interval.clone().into(),
            ],
        ))
        .await?;
    let mut decisions = rows.iter().map(decision).collect::<Result<Vec<_>, _>>()?;
    decisions.reverse();
    let iv = dec::interval_us(interval).unwrap_or(DAY_US);
    let at = |d: &Decision| {
        DateTime::parse_from_rfc3339(&d.as_of)
            .map(|t| t.timestamp_micros())
            .unwrap_or_default()
    };
    // One read at most as wide as the bars route allows: older decisions of
    // the group are left out of the measure (counted in `decisions` only).
    let total = decisions.len();
    if let Some(newest) = decisions.last().map(at) {
        let oldest = newest - dec::BARS_MAX as i64 * iv;
        decisions.retain(|d| at(d) >= oldest);
    }
    let mut bars = Vec::new();
    if let (Some(table), Some(first), Some(last)) = (table, decisions.first(), decisions.last()) {
        let atr = decisions
            .iter()
            .filter_map(|d| d.barriers.map(|b| b.atr_period))
            .max()
            .unwrap_or(0) as i64;
        let horizon = decisions.iter().map(|d| d.horizon_bars).max().unwrap_or(0) as i64;
        let now = Utc::now().timestamp_micros();
        let q = BarsQuery {
            instrument,
            source,
            interval_us: iv,
            from_us: Some(at(first) - (atr + 1) * iv * 2 - 4 * DAY_US),
            to_us: Some(at(last) + horizon * iv * 2 + 4 * DAY_US),
            limit: usize::MAX,
            as_of_us: now,
        };
        bars = self::bars(db, store, org, table, &q).await?;
    }
    let items: Vec<(Vec<f64>, Option<dec::Outcome>)> = decisions
        .iter()
        .map(|d| {
            let t = at(d);
            let split_before = bars.partition_point(|b| b.ts_us + iv <= t);
            let split_after = bars.partition_point(|b| b.ts_us < t);
            let out = dec::outcome(
                &bars[..split_before],
                &bars[split_after..],
                d.horizon_bars as usize,
                d.barriers.as_ref(),
            );
            (d.probabilities.clone(), out)
        })
        .collect();
    Ok(ModelScores {
        model: model.clone(),
        model_version: version.clone(),
        instrument: instrument.clone(),
        source: source.clone(),
        interval: interval.clone(),
        scores: dec::Scores {
            decisions: total as u64,
            ..dec::scores(&items, threshold)
        },
    })
}

/// Last decision of each (day, model, version) of an instrument, for the
/// chart (D897), oldest first.
pub async fn chart_markers<C: ConnectionTrait>(
    db: &C,
    org: i64,
    instrument: &str,
    since: chrono::NaiveDate,
) -> Result<Vec<pnex_core::markets::chart::ChartEvent>, DbErr> {
    let rows = db
        .query_all_raw(stmt(
            "SELECT * FROM (SELECT DISTINCT ON ((as_of AT TIME ZONE 'UTC')::date, model, model_version) \
               id, (as_of AT TIME ZONE 'UTC')::date::text AS d, model, model_version, best, probabilities \
             FROM market_decisions WHERE org_id = $1 AND instrument = $2 AND as_of >= $3::date \
             ORDER BY (as_of AT TIME ZONE 'UTC')::date, model, model_version, as_of DESC) x \
             ORDER BY d LIMIT 2000",
            vec![org.into(), instrument.into(), since.to_string().into()],
        ))
        .await?;
    rows.iter()
        .map(|r| {
            let best: String = r.try_get("", "best")?;
            let p: Vec<f64> = r.try_get("", "probabilities")?;
            let model = format!(
                "{} {}",
                r.try_get::<String>("", "model")?,
                r.try_get::<String>("", "model_version")?
            );
            Ok(pnex_core::markets::chart::ChartEvent {
                id: r.try_get::<Uuid>("", "id")?.to_string(),
                date: r.try_get("", "d")?,
                kind: pnex_core::markets::chart::ChartEventKind::Decision,
                title: model.clone(),
                probability: OPTIONS
                    .iter()
                    .position(|o| *o == best)
                    .and_then(|i| p.get(i).copied()),
                detail: best,
                model: Some(model),
            })
        })
        .collect()
}
