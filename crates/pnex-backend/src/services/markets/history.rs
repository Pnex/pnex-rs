//! Fine price history into a table (markets-research.md MK11–MK12:
//! D807–D809, D820, D822): the 1-minute bars of the Binance symbols and the
//! ticks of the Dukascopy pairs an org lists, read from the public files
//! one partition at a time, each one an `append` transaction of the
//! source's table. The registry `market_history_partitions` makes a cut run
//! go on where it stopped.

use std::collections::HashSet;
use std::sync::Arc;

use arrow_array::{
    Decimal128Array, Float64Array, Int64Array, RecordBatch, StringArray, TimestampMicrosecondArray,
};
use arrow_schema::{DataType, Field, Schema, TimeUnit};
use chrono::NaiveDate;
use loco_rs::app::AppContext;
use pnex_core::markets::history::{
    self, HistorySource, Kline, Period, Tick, BINANCE_SPOT_1M, DUKASCOPY_TICKS,
};
use sea_orm::{ConnectionTrait, DatabaseBackend, DatabaseConnection, Statement};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::services::artifact_store::S3Store;
use crate::services::datasets::{self, DatasetError};
use crate::services::ontology::imports::source;

const SYMBOLS: &str = "markets.history_symbols";
const FROM: &str = "markets.history_from";
/// The first month Binance has spot files for.
const FROM_DEFAULT: &str = "2017-08";
const SYMBOLS_MAX: usize = 50;
/// Partitions fetched per org and pass: the backfill spreads over passes.
// shortcut: one request at a time per pass, no per-host concurrency (D808 says 4); raise it if a backfill measured too slow.
const PER_PASS: usize = 24;
const PAUSE: std::time::Duration = std::time::Duration::from_millis(250);
const FX_PAIRS: &str = "markets.history_fx_pairs";
const FX_FROM: &str = "markets.history_fx_from";
/// Days of ticks per org and pass (24 hour files each).
const FX_DAYS_PER_PASS: usize = 14;
const FX_PAUSE: std::time::Duration = std::time::Duration::from_millis(100);

fn stmt(sql: &str, values: Vec<sea_orm::Value>) -> Statement {
    Statement::from_sql_and_values(DatabaseBackend::Postgres, sql, values)
}

/// One pass over the orgs that list symbols. `base` is the source's host
/// (a mock in tests).
pub async fn sweep(ctx: &AppContext, base: &str, today: NaiveDate) -> Result<(), DatasetError> {
    sweep_from(ctx, base, DUKASCOPY_TICKS.base, today).await
}

/// [`sweep`] with each source's host (mocks in tests).
pub async fn sweep_from(
    ctx: &AppContext,
    binance: &str,
    dukascopy: &str,
    today: NaiveDate,
) -> Result<(), DatasetError> {
    let orgs: Vec<i64> = ctx
        .db
        .query_all_raw(stmt(
            "SELECT org_id FROM packs WHERE key = 'markets' \
               AND (coalesce(settings ->> 'history_symbols', '') <> '' \
                 OR coalesce(settings ->> 'history_fx_pairs', '') <> '') ORDER BY org_id",
            vec![],
        ))
        .await?
        .iter()
        .filter_map(|r| r.try_get("", "org_id").ok())
        .collect();
    if orgs.is_empty() {
        return Ok(());
    }
    let store = datasets::store_of(&ctx.config)?;
    for org in orgs {
        if let Err(e) = org_pass(&ctx.db, &store, binance, org, today).await {
            tracing::warn!(org, code = e.code(), "price history pass stopped");
        }
        if let Err(e) = fx_pass(&ctx.db, &store, dukascopy, org, today).await {
            tracing::warn!(org, code = e.code(), "tick history pass stopped");
        }
    }
    Ok(())
}

async fn text_setting(
    db: &DatabaseConnection,
    org: i64,
    key: &str,
) -> Result<String, DatasetError> {
    Ok(
        crate::services::ontology::packs::setting_value(db, org, key)
            .await?
            .and_then(|v| v.as_str().map(str::to_string))
            .unwrap_or_default(),
    )
}

/// The symbols of a setting: upper-cased, those unsafe in a path left out.
fn symbols_of(raw: &str) -> Vec<String> {
    let mut seen = HashSet::new();
    raw.split(',')
        .map(|s| s.trim().to_ascii_uppercase())
        .filter(|s| history::symbol_ok(s) && seen.insert(s.clone()))
        .take(SYMBOLS_MAX)
        .collect()
}

/// The forex pairs of a setting whose scale is known (D820).
fn pairs_of(raw: &str) -> Vec<String> {
    let mut seen = HashSet::new();
    raw.split(',')
        .map(|s| s.trim().to_ascii_uppercase())
        .filter(|s| history::fx_scale(s).is_some() && seen.insert(s.clone()))
        .take(SYMBOLS_MAX)
        .collect()
}

/// A `YYYY-MM` setting, else twelve months back.
fn fx_from_of(raw: &str, today: NaiveDate) -> NaiveDate {
    NaiveDate::parse_from_str(&format!("{}-01", raw.trim()), "%Y-%m-%d").unwrap_or_else(|_| {
        today
            .checked_sub_months(chrono::Months::new(12))
            .unwrap_or(today)
    })
}

fn from_of(raw: &str) -> NaiveDate {
    let raw = if raw.trim().is_empty() {
        FROM_DEFAULT
    } else {
        raw.trim()
    };
    NaiveDate::parse_from_str(&format!("{raw}-01"), "%Y-%m-%d")
        .unwrap_or_else(|_| NaiveDate::from_ymd_opt(2017, 8, 1).unwrap_or_default())
}

async fn org_pass(
    db: &DatabaseConnection,
    store: &S3Store,
    base: &str,
    org: i64,
    today: NaiveDate,
) -> Result<(), DatasetError> {
    let symbols = symbols_of(&text_setting(db, org, SYMBOLS).await?);
    let from = from_of(&text_setting(db, org, FROM).await?);
    let src = BINANCE_SPOT_1M;
    let mut budget = PER_PASS;
    for symbol in symbols {
        let known = settled(db, org, &src, &symbol, today).await?;
        for p in history::plan(from, today, &known) {
            if budget == 0 {
                return Ok(());
            }
            budget -= 1;
            // An unreachable source stops the pass, nothing is counted.
            let Some(outcome) = fetch(base, &src, &symbol, p).await else {
                return Ok(());
            };
            let outcome = match outcome {
                Fetched::Rows(batch, sha256) => write(db, store, org, &src, batch, sha256).await,
                other => other,
            };
            let stop = matches!(&outcome, Fetched::Failed("quota"));
            record(db, org, &src, &symbol, p, &outcome).await?;
            if stop {
                return Err(DatasetError::QuotaExceeded);
            }
            tokio::time::sleep(PAUSE).await;
        }
    }
    Ok(())
}

async fn fx_pass(
    db: &DatabaseConnection,
    store: &S3Store,
    base: &str,
    org: i64,
    today: NaiveDate,
) -> Result<(), DatasetError> {
    let pairs = pairs_of(&text_setting(db, org, FX_PAIRS).await?);
    let from = fx_from_of(&text_setting(db, org, FX_FROM).await?, today);
    let src = DUKASCOPY_TICKS;
    let mut budget = FX_DAYS_PER_PASS;
    for pair in pairs {
        let known = settled(db, org, &src, &pair, today).await?;
        for p in history::plan_days(from, today, &known) {
            if budget == 0 {
                return Ok(());
            }
            budget -= 1;
            let Some(outcome) = fetch_day(base, &pair, p).await else {
                return Ok(());
            };
            let outcome = match outcome {
                Fetched::Rows(batch, sha256) => write(db, store, org, &src, batch, sha256).await,
                other => other,
            };
            let stop = matches!(&outcome, Fetched::Failed("quota"));
            record(db, org, &src, &pair, p, &outcome).await?;
            if stop {
                return Err(DatasetError::QuotaExceeded);
            }
        }
    }
    Ok(())
}

/// The 24 hour files of a day, one batch. An hour without ticks is a 404
/// or an empty file; a day without any is `Empty`.
async fn fetch_day(base: &str, pair: &str, p: Period) -> Option<Fetched> {
    let day = p.start();
    let scale = history::fx_scale(pair)?;
    let by = source::Declared::default();
    let mut ticks: Vec<Tick> = Vec::new();
    let mut digest = Sha256::new();
    for hour in 0..24 {
        let path = history::dukascopy_path(pair, day, hour);
        let url = source::check_url(&format!("{base}/{path}")).ok()?;
        let body = match source::get_found(&url, &by).await {
            Ok(Some(b)) => b,
            Ok(None) => continue,
            Err(code) => {
                tracing::info!(%url, code, "tick history source not read");
                return None;
            }
        };
        tokio::time::sleep(FX_PAUSE).await;
        if body.is_empty() {
            continue;
        }
        digest.update(&body);
        let start_us = day.and_hms_opt(hour, 0, 0)?.and_utc().timestamp_micros();
        let decoded = tokio::task::spawn_blocking(move || {
            let raw = pnex_core::import_read::unlzma(&body).ok()?;
            history::dukascopy_bi5(&raw, start_us, scale).ok()
        })
        .await
        .ok()
        .flatten();
        match decoded {
            Some(t) => ticks.extend(t),
            None => return Some(Fetched::Failed("unreadable")),
        }
    }
    if ticks.is_empty() {
        return Some(Fetched::Empty);
    }
    let sha256: String = digest
        .finalize()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    Some(match quote_tick_batch(&DUKASCOPY_TICKS, pair, &ticks) {
        Ok(b) => Fetched::Rows(b, sha256),
        Err(_) => Fetched::Failed("unreadable"),
    })
}

async fn settled(
    db: &DatabaseConnection,
    org: i64,
    src: &HistorySource,
    symbol: &str,
    today: NaiveDate,
) -> Result<HashSet<Period>, DatasetError> {
    let rows = db
        .query_all_raw(stmt(
            "SELECT period, period_start, state FROM market_history_partitions \
             WHERE org_id = $1 AND source = $2 AND instrument = $3",
            vec![org.into(), src.key.into(), symbol.into()],
        ))
        .await?;
    let mut out = HashSet::new();
    for r in rows {
        let kind: String = r.try_get("", "period")?;
        let start: NaiveDate = r.try_get("", "period_start")?;
        let state: String = r.try_get("", "state")?;
        if let Some(p) = Period::of(&kind, start).filter(|p| p.settled(&state, today)) {
            out.insert(p);
        }
    }
    Ok(out)
}

enum Fetched {
    Rows(RecordBatch, String),
    Empty,
    Failed(&'static str),
    Done {
        transaction: Uuid,
        rows: i64,
        bytes: i64,
        sha256: String,
    },
}

/// `None` when the source did not answer (down, 429, 503): try again later.
async fn fetch(base: &str, src: &HistorySource, symbol: &str, p: Period) -> Option<Fetched> {
    let path = history::binance_klines_path(symbol, src.interval, p);
    let url = source::check_url(&format!("{base}/{path}")).ok()?;
    let by = source::Declared::default();
    let zip = match source::get_found(&url, &by).await {
        Ok(Some(z)) => z,
        Ok(None) => return Some(Fetched::Empty),
        Err(code) => {
            tracing::info!(%url, code, "price history source not read");
            return None;
        }
    };
    let sha256: String = Sha256::digest(&zip)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    let check_url = source::check_url(&format!("{url}.CHECKSUM")).ok()?;
    match source::get_found(&check_url, &by).await {
        Ok(Some(text)) => {
            let want = history::checksum_of(&String::from_utf8_lossy(&text));
            if want.as_deref() != Some(sha256.as_str()) {
                return Some(Fetched::Failed("checksum"));
            }
        }
        // shortcut: a file published without its checksum is loaded unchecked; refuse it if Binance ever stops publishing them.
        Ok(None) => {}
        Err(_) => return None,
    }
    let decoded = tokio::task::spawn_blocking(move || {
        let csv = pnex_core::import_read::unzip_csv(&zip).ok()?;
        history::binance_klines(&csv).ok()
    })
    .await
    .ok()
    .flatten();
    Some(match decoded {
        None => Fetched::Failed("unreadable"),
        Some(rows) if rows.is_empty() => Fetched::Empty,
        Some(rows) => match ohlcv_batch(src, symbol, &rows) {
            Ok(b) => Fetched::Rows(b, sha256),
            Err(_) => Fetched::Failed("unreadable"),
        },
    })
}

/// Columns of the `ohlcv` shape (D809).
pub fn ohlcv_batch(
    src: &HistorySource,
    symbol: &str,
    rows: &[Kline],
) -> Result<RecordBatch, arrow_schema::ArrowError> {
    let scale = history::SCALE as i8;
    let dec = || DataType::Decimal128(38, scale);
    let ts = DataType::Timestamp(TimeUnit::Microsecond, Some("UTC".into()));
    let schema = Arc::new(Schema::new(vec![
        Field::new("instrument", DataType::Utf8, false),
        Field::new("ts", ts, false),
        Field::new("interval", DataType::Utf8, false),
        Field::new("open", dec(), false),
        Field::new("high", dec(), false),
        Field::new("low", dec(), false),
        Field::new("close", dec(), false),
        Field::new("volume", dec(), false),
        Field::new("quote_volume", dec(), true),
        Field::new("trades", DataType::Int64, true),
        Field::new("source", DataType::Utf8, false),
    ]));
    let n = rows.len();
    let col = |f: fn(&Kline) -> i128| {
        Decimal128Array::from(rows.iter().map(f).collect::<Vec<_>>())
            .with_precision_and_scale(38, scale)
    };
    RecordBatch::try_new(
        schema,
        vec![
            Arc::new(StringArray::from(vec![symbol; n])),
            Arc::new(
                TimestampMicrosecondArray::from(rows.iter().map(|k| k.open_us).collect::<Vec<_>>())
                    .with_timezone("UTC"),
            ),
            Arc::new(StringArray::from(vec![src.interval; n])),
            Arc::new(col(|k| k.open)?),
            Arc::new(col(|k| k.high)?),
            Arc::new(col(|k| k.low)?),
            Arc::new(col(|k| k.close)?),
            Arc::new(col(|k| k.volume)?),
            Arc::new(col(|k| k.quote_volume)?),
            Arc::new(Int64Array::from(
                rows.iter().map(|k| k.trades).collect::<Vec<_>>(),
            )),
            Arc::new(StringArray::from(vec![src.key; n])),
        ],
    )
}

/// Columns of the `quote_tick` shape (D809).
pub fn quote_tick_batch(
    src: &HistorySource,
    instrument: &str,
    rows: &[Tick],
) -> Result<RecordBatch, arrow_schema::ArrowError> {
    let scale = history::SCALE as i8;
    let dec = || DataType::Decimal128(38, scale);
    let ts = DataType::Timestamp(TimeUnit::Microsecond, Some("UTC".into()));
    let schema = Arc::new(Schema::new(vec![
        Field::new("instrument", DataType::Utf8, false),
        Field::new("ts", ts, false),
        Field::new("bid", dec(), false),
        Field::new("ask", dec(), false),
        Field::new("bid_volume", DataType::Float64, false),
        Field::new("ask_volume", DataType::Float64, false),
        Field::new("source", DataType::Utf8, false),
    ]));
    let n = rows.len();
    let col = |f: fn(&Tick) -> i128| {
        Decimal128Array::from(rows.iter().map(f).collect::<Vec<_>>())
            .with_precision_and_scale(38, scale)
    };
    RecordBatch::try_new(
        schema,
        vec![
            Arc::new(StringArray::from(vec![instrument; n])),
            Arc::new(
                TimestampMicrosecondArray::from(rows.iter().map(|t| t.ts_us).collect::<Vec<_>>())
                    .with_timezone("UTC"),
            ),
            Arc::new(col(|t| t.bid)?),
            Arc::new(col(|t| t.ask)?),
            Arc::new(Float64Array::from(
                rows.iter().map(|t| t.bid_volume).collect::<Vec<_>>(),
            )),
            Arc::new(Float64Array::from(
                rows.iter().map(|t| t.ask_volume).collect::<Vec<_>>(),
            )),
            Arc::new(StringArray::from(vec![src.key; n])),
        ],
    )
}

/// The org's table of this source, created the first time.
async fn table_of(
    db: &DatabaseConnection,
    org: i64,
    src: &HistorySource,
) -> Result<Uuid, DatasetError> {
    let find = || async {
        db.query_one_raw(stmt(
            "SELECT id FROM datasets WHERE org_id = $1 AND name = $2",
            vec![org.into(), src.table.into()],
        ))
        .await?
        .map(|r| r.try_get::<Uuid>("", "id"))
        .transpose()
    };
    if let Some(id) = find().await? {
        return Ok(id);
    }
    match datasets::create(db, org, src.table).await {
        Ok(d) => d.id.parse().map_err(|_| DatasetError::NotFound),
        // Created meanwhile by another pod.
        Err(DatasetError::NameTaken) => find().await?.ok_or(DatasetError::NotFound),
        Err(e) => Err(e),
    }
}

async fn write(
    db: &DatabaseConnection,
    store: &S3Store,
    org: i64,
    src: &HistorySource,
    batch: RecordBatch,
    sha256: String,
) -> Fetched {
    let out = async {
        let table = table_of(db, org, src).await?;
        datasets::append_batch(db, store, org, table, &batch).await
    }
    .await;
    match out {
        Ok(t) => Fetched::Done {
            transaction: t.id.parse().unwrap_or_default(),
            rows: t.rows,
            bytes: t.bytes,
            sha256,
        },
        Err(DatasetError::QuotaExceeded) => Fetched::Failed("quota"),
        Err(DatasetError::SchemaMismatch { .. }) => Fetched::Failed("schema"),
        Err(e) => {
            let detail = match &e {
                DatasetError::Storage(m) => m.clone(),
                DatasetError::Db(d) => d.to_string(),
                _ => String::new(),
            };
            tracing::warn!(org, table = src.table, code = e.code(), %detail, "price history not written");
            Fetched::Failed("storage")
        }
    }
}

async fn record(
    db: &DatabaseConnection,
    org: i64,
    src: &HistorySource,
    symbol: &str,
    p: Period,
    outcome: &Fetched,
) -> Result<(), DatasetError> {
    let (state, code, rows, bytes, sha, tx): (
        &str,
        Option<&str>,
        i64,
        i64,
        Option<String>,
        Option<Uuid>,
    ) = match outcome {
        Fetched::Done {
            transaction,
            rows,
            bytes,
            sha256,
        } => (
            "done",
            None,
            *rows,
            *bytes,
            Some(sha256.clone()),
            Some(*transaction),
        ),
        Fetched::Empty => ("empty", None, 0, 0, None, None),
        Fetched::Failed(c) => ("failed", Some(*c), 0, 0, None, None),
        Fetched::Rows(..) => return Ok(()),
    };
    db.execute_raw(stmt(
        "INSERT INTO market_history_partitions \
           (org_id, source, instrument, period, period_start, state, code, rows, bytes, sha256, transaction_id) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11) \
         ON CONFLICT (org_id, source, instrument, period, period_start) DO UPDATE SET \
           state = EXCLUDED.state, code = EXCLUDED.code, rows = EXCLUDED.rows, bytes = EXCLUDED.bytes, \
           sha256 = EXCLUDED.sha256, transaction_id = EXCLUDED.transaction_id, fetched_at = now()",
        vec![
            org.into(),
            src.key.into(),
            symbol.into(),
            p.kind().into(),
            p.start().into(),
            state.into(),
            code.map(str::to_string).into(),
            rows.into(),
            bytes.into(),
            sha.into(),
            tx.into(),
        ],
    ))
    .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings_are_read_safely() {
        assert_eq!(
            symbols_of(" btcusdt, ETHUSDT ,../x,,BTCUSDT"),
            vec!["BTCUSDT", "ETHUSDT"]
        );
        assert_eq!(from_of(""), NaiveDate::from_ymd_opt(2017, 8, 1).unwrap());
        assert_eq!(
            from_of("2024-03"),
            NaiveDate::from_ymd_opt(2024, 3, 1).unwrap()
        );
        assert_eq!(
            from_of("junk"),
            NaiveDate::from_ymd_opt(2017, 8, 1).unwrap()
        );
        assert_eq!(
            pairs_of("eurusd, XAUUSD,usdjpy,EURUSD"),
            vec!["EURUSD", "USDJPY"]
        );
        let today = NaiveDate::from_ymd_opt(2026, 11, 8).unwrap();
        assert_eq!(
            fx_from_of("", today),
            NaiveDate::from_ymd_opt(2025, 11, 8).unwrap()
        );
        assert_eq!(
            fx_from_of("2025-03", today),
            NaiveDate::from_ymd_opt(2025, 3, 1).unwrap()
        );
    }
}
