//! Fine price history into a table (markets-research.md MK11–MK12:
//! D807–D809, D820, D822): the 1-minute bars and the trades of the Binance
//! symbols and the ticks of the Dukascopy pairs an org lists, read from the
//! public files
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
    self, HistorySource, Kline, Period, Tick, Trade, BINANCE_SPOT_1M, BINANCE_SPOT_TRADES,
    DUKASCOPY_TICKS, HISTDATA_M1, HISTDATA_TICKS, MT5_OHLCV_1M, MT5_TICKS,
};
use sea_orm::{ConnectionTrait, DatabaseBackend, DatabaseConnection, Statement};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::services::artifact_store::S3Store;
use crate::services::datasets::{self, DatasetError};
use crate::services::db_lock::{ns, TenantLock};
use crate::services::ontology::imports::source;

const SYMBOLS_MAX: usize = 50;
// shortcut: one request at a time per pass, no per-host concurrency (D808 says 4); raise it if a backfill measured too slow.
const PAUSE: std::time::Duration = std::time::Duration::from_millis(250);
const FX_PAUSE: std::time::Duration = std::time::Duration::from_millis(100);
const LOCK_WAIT: std::time::Duration = std::time::Duration::from_secs(1);
/// UTC as an offset: a named zone needs `chrono-tz` to be read back (the
/// table sample formats values with arrow).
pub(crate) const UTC: &str = "+00:00";

/// What a pass loads: one source, one shape, its two settings.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Job {
    /// Binance spot 1-minute bars (MK11).
    Klines,
    /// Dukascopy forex ticks (MK12).
    Ticks,
    /// Binance spot trades (MK13).
    Trades,
    /// HistData 1-minute bars: a year per file, one file at a time.
    HistData,
    /// HistData ticks: a month per file, read as a stream.
    HistDataTicks,
}

const JOBS: [Job; 5] = [
    Job::Klines,
    Job::Ticks,
    Job::Trades,
    Job::HistData,
    Job::HistDataTicks,
];

impl Job {
    fn src(self) -> HistorySource {
        match self {
            Self::Klines => BINANCE_SPOT_1M,
            Self::Ticks => DUKASCOPY_TICKS,
            Self::Trades => BINANCE_SPOT_TRADES,
            Self::HistData => HISTDATA_M1,
            Self::HistDataTicks => HISTDATA_TICKS,
        }
    }

    /// The settings of the instruments and of the first month.
    fn settings(self) -> (&'static str, &'static str) {
        match self {
            Self::Klines => ("history_symbols", "history_from"),
            Self::Ticks => ("history_fx_pairs", "history_fx_from"),
            Self::Trades => ("history_trade_symbols", "history_trade_from"),
            Self::HistData => ("histdata_pairs", "histdata_from"),
            Self::HistDataTicks => ("histdata_tick_pairs", "histdata_tick_from"),
        }
    }

    /// Partitions per org and pass: the backfill spreads over passes.
    fn budget(self) -> usize {
        match self {
            Self::Klines => 24,
            // 24 hour files each.
            Self::Ticks => 14,
            // Up to a few hundred MB unzipped each.
            Self::Trades => 4,
            // A free site: one file per pass, as a person would.
            Self::HistData | Self::HistDataTicks => 1,
        }
    }

    /// Pause after each partition.
    fn pause(self) -> std::time::Duration {
        match self {
            Self::HistData | Self::HistDataTicks => std::time::Duration::from_secs(5),
            _ => PAUSE,
        }
    }

    fn instruments(self, raw: &str) -> Vec<String> {
        let ok: fn(&str) -> bool = match self {
            Self::Ticks => |s| history::fx_scale(s).is_some(),
            Self::Klines | Self::Trades | Self::HistData | Self::HistDataTicks => {
                history::symbol_ok
            }
        };
        let mut seen = HashSet::new();
        raw.split(',')
            .map(|s| s.trim().to_ascii_uppercase())
            .filter(|s| ok(s) && seen.insert(s.clone()))
            .take(SYMBOLS_MAX)
            .collect()
    }

    /// A `YYYY-MM` setting, else the job's default: Binance's first month
    /// for bars, twelve months back for ticks, one for trades.
    fn from(self, raw: &str, today: NaiveDate) -> NaiveDate {
        if let Ok(d) = NaiveDate::parse_from_str(&format!("{}-01", raw.trim()), "%Y-%m-%d") {
            return d;
        }
        if let Ok(y) = raw.trim().parse::<i32>() {
            if let Some(d) = NaiveDate::from_ymd_opt(y, 1, 1) {
                return d;
            }
        }
        let back = |months| {
            today
                .checked_sub_months(chrono::Months::new(months))
                .unwrap_or(today)
        };
        match self {
            Self::Klines => NaiveDate::from_ymd_opt(2017, 8, 1).unwrap_or(today),
            Self::Ticks => back(12),
            Self::Trades => back(1),
            // HistData's first year.
            Self::HistData => NaiveDate::from_ymd_opt(2000, 1, 1).unwrap_or(today),
            // Ticks weigh tens of MB a month: one month back unless set.
            Self::HistDataTicks => back(1),
        }
    }

    fn plan(self, from: NaiveDate, today: NaiveDate, known: &HashSet<Period>) -> Vec<Period> {
        match self {
            Self::Klines => history::plan(from, today, known),
            Self::Ticks | Self::Trades => history::plan_days(from, today, known),
            Self::HistData => history::plan_years(from, today, known),
            Self::HistDataTicks => history::plan_months(from, today, known),
        }
    }

    /// `None` when the source did not answer (down, 429, 503): try again later.
    async fn fetch(self, base: &str, instrument: &str, p: Period) -> Option<Fetched> {
        match self {
            Self::Klines => {
                let path = history::binance_klines_path(instrument, BINANCE_SPOT_1M.interval, p);
                fetch_binance(base, &path, instrument, klines_of).await
            }
            Self::Trades => {
                let path = history::binance_trades_path(instrument, p.start());
                fetch_binance(base, &path, instrument, trades_of).await
            }
            Self::Ticks => fetch_day(base, instrument, p).await,
            Self::HistData => fetch_histdata(base, instrument, p).await,
            // Streamed into the table by `fetch_histdata_ticks`.
            Self::HistDataTicks => Some(Fetched::Failed("unreadable")),
        }
    }
}

fn stmt(sql: &str, values: Vec<sea_orm::Value>) -> Statement {
    Statement::from_sql_and_values(DatabaseBackend::Postgres, sql, values)
}

/// The host of each source (mocks in tests).
#[derive(Debug, Clone)]
pub struct Hosts {
    pub binance: String,
    pub dukascopy: String,
    pub histdata: String,
}

impl Default for Hosts {
    fn default() -> Self {
        Self {
            binance: BINANCE_SPOT_1M.base.into(),
            dukascopy: DUKASCOPY_TICKS.base.into(),
            histdata: HISTDATA_M1.base.into(),
        }
    }
}

impl Hosts {
    fn of(&self, job: Job) -> &str {
        match job {
            Job::Klines | Job::Trades => &self.binance,
            Job::Ticks => &self.dukascopy,
            Job::HistData | Job::HistDataTicks => &self.histdata,
        }
    }
}

/// One pass over the orgs that list instruments, on the real hosts.
pub async fn sweep(ctx: &AppContext, today: NaiveDate) -> Result<(), DatasetError> {
    sweep_from(ctx, &Hosts::default(), today).await
}

/// [`sweep`] with each source's host (mocks in tests).
pub async fn sweep_from(
    ctx: &AppContext,
    hosts: &Hosts,
    today: NaiveDate,
) -> Result<(), DatasetError> {
    let orgs: Vec<i64> = ctx
        .db
        .query_all_raw(stmt(
            "SELECT org_id FROM packs WHERE key = 'markets' \
               AND (coalesce(settings ->> 'history_symbols', '') <> '' \
                 OR coalesce(settings ->> 'history_fx_pairs', '') <> '' \
                 OR coalesce(settings ->> 'history_trade_symbols', '') <> '' \
                 OR coalesce(settings ->> 'histdata_pairs', '') <> '' \
                 OR coalesce(settings ->> 'histdata_tick_pairs', '') <> '' \
                 OR EXISTS (SELECT 1 FROM mt5_inbox i WHERE i.org_id = packs.org_id)) \
             UNION SELECT DISTINCT org_id FROM mt5_inbox ORDER BY org_id",
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
        // A pass outliving its sweep turn must not run beside the next one:
        // both would read the registry before the other writes it.
        let Ok(lock) = TenantLock::acquire(&ctx.db, ns::MARKET_HISTORY, org, LOCK_WAIT).await
        else {
            tracing::info!(org, "price history pass already running");
            continue;
        };
        for job in JOBS {
            if let Err(e) = pass(&ctx.db, &store, hosts.of(job), org, today, job).await {
                tracing::warn!(org, ?job, code = e.code(), "price history pass stopped");
            }
        }
        if let Err(e) = mt5_pass(&ctx.db, &store, org).await {
            tracing::warn!(org, code = e.code(), "MT5 terminals pass stopped");
        }
        lock.release().await;
    }
    Ok(())
}

async fn text_setting(
    db: &DatabaseConnection,
    org: i64,
    key: &str,
) -> Result<String, DatasetError> {
    Ok(
        crate::services::ontology::packs::setting_value(db, org, &format!("markets.{key}"))
            .await?
            .and_then(|v| v.as_str().map(str::to_string))
            .unwrap_or_default(),
    )
}

/// Calls of the MT5 terminals read per pass, oldest first.
const MT5_CALLS_PER_PASS: i64 = 1000;

/// Empties the inbox of the org's MT5 terminals (D849) into `mt5_ohlcv_1m`
/// and `mt5_ticks`: one transaction per table and source, then the calls
/// read leave the inbox. An unreadable call is dropped.
// shortcut: a pod killed between the commit and the delete loads those calls again (rows twice); key the calls in the registry if that is ever seen.
async fn mt5_pass(db: &DatabaseConnection, store: &S3Store, org: i64) -> Result<(), DatasetError> {
    loop {
        // In the org (R1): the inbox row carries the org of its terminal.
        let calls = db
            .query_all_raw(stmt(
                "SELECT id, body FROM mt5_inbox WHERE org_id = $1 ORDER BY id LIMIT $2",
                vec![org.into(), MT5_CALLS_PER_PASS.into()],
            ))
            .await?;
        let Some(last) = calls
            .last()
            .map(|r| r.try_get::<i64>("", "id"))
            .transpose()?
        else {
            return Ok(());
        };
        let mut bars: std::collections::BTreeMap<String, Vec<(String, Kline)>> = Default::default();
        let mut ticks: std::collections::BTreeMap<String, Vec<(String, Tick)>> = Default::default();
        let mut digest = Sha256::new();
        for call in &calls {
            let body: serde_json::Value = call.try_get("", "body")?;
            match history::mt5_payload(&body) {
                Ok(b) => {
                    digest.update(body.to_string().as_bytes());
                    bars.entry(b.source.clone()).or_default().extend(b.bars);
                    ticks.entry(b.source).or_default().extend(b.ticks);
                }
                Err(token) => tracing::warn!(org, token, "MT5 terminal call dropped"),
            }
        }
        let sha256 = hex(&digest.finalize());
        for (source, rows) in bars.into_iter().filter(|(_, r)| !r.is_empty()) {
            let names: Vec<&str> = rows.iter().map(|(s, _)| s.as_str()).collect();
            let klines: Vec<Kline> = rows.iter().map(|(_, k)| k.clone()).collect();
            let batch = ohlcv_batch(MT5_OHLCV_1M.interval, &source, &names, &klines)
                .map_err(|e| DatasetError::Storage(e.to_string()))?;
            mt5_write(db, store, org, &MT5_OHLCV_1M, batch, &sha256).await?;
        }
        for (source, rows) in ticks.into_iter().filter(|(_, r)| !r.is_empty()) {
            let names: Vec<&str> = rows.iter().map(|(s, _)| s.as_str()).collect();
            let quotes: Vec<Tick> = rows.iter().map(|(_, t)| t.clone()).collect();
            let batch = quote_tick_batch(&source, &names, &quotes)
                .map_err(|e| DatasetError::Storage(e.to_string()))?;
            mt5_write(db, store, org, &MT5_TICKS, batch, &sha256).await?;
        }
        db.execute_raw(stmt(
            "DELETE FROM mt5_inbox WHERE org_id = $1 AND id <= $2",
            vec![org.into(), last.into()],
        ))
        .await?;
        if (calls.len() as i64) < MT5_CALLS_PER_PASS {
            return Ok(());
        }
    }
}

/// [`write`] for the bridge: a refusal stops the pass and keeps the calls.
async fn mt5_write(
    db: &DatabaseConnection,
    store: &S3Store,
    org: i64,
    src: &HistorySource,
    batch: RecordBatch,
    sha256: &str,
) -> Result<(), DatasetError> {
    match write(db, store, org, src, batch, sha256.to_string()).await {
        Fetched::Done { .. } => Ok(()),
        Fetched::Failed("quota") => Err(DatasetError::QuotaExceeded),
        _ => Err(DatasetError::Storage("MT5 rows not written".into())),
    }
}

async fn pass(
    db: &DatabaseConnection,
    store: &S3Store,
    base: &str,
    org: i64,
    today: NaiveDate,
    job: Job,
) -> Result<(), DatasetError> {
    let (items, from) = job.settings();
    let items = job.instruments(&text_setting(db, org, items).await?);
    if items.is_empty() {
        return Ok(());
    }
    let from = job.from(&text_setting(db, org, from).await?, today);
    let src = job.src();
    // Never asked first, over every instrument: a partition asked again
    // (empty or failed) of one pair must not hold back another pair's
    // first files when the budget is one file.
    let (mut fresh, mut again) = (Vec::new(), Vec::new());
    for instrument in items {
        let (known, seen) = settled(db, org, &src, &instrument, today).await?;
        for p in job.plan(from, today, &known) {
            let list = if seen.contains(&p) {
                &mut again
            } else {
                &mut fresh
            };
            list.push((instrument.clone(), p));
        }
    }
    for (instrument, p) in fresh.into_iter().chain(again).take(job.budget()) {
        // An unreachable source stops the pass, nothing is counted.
        let fetched = match job {
            Job::HistDataTicks => fetch_histdata_ticks(db, store, org, base, &instrument, p).await,
            _ => job.fetch(base, &instrument, p).await,
        };
        let Some(outcome) = fetched else {
            return Ok(());
        };
        let outcome = match outcome {
            Fetched::Rows(batch, sha256) => write(db, store, org, &src, batch, sha256).await,
            other => other,
        };
        let stop = matches!(&outcome, Fetched::Failed("quota"));
        record(db, org, &src, &instrument, p, &outcome).await?;
        if stop {
            return Err(DatasetError::QuotaExceeded);
        }
        tokio::time::sleep(job.pause()).await;
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
    Some(
        match quote_tick_batch(DUKASCOPY_TICKS.key, &vec![pair; ticks.len()], &ticks) {
            Ok(b) => Fetched::Rows(b, hex(&digest.finalize())),
            Err(_) => Fetched::Failed("unreadable"),
        },
    )
}

/// A HistData file: its page, then the page's own download form sent back
/// with the page as referer. A page without a file is `Empty` (a year
/// before the pair's first, or not published yet).
async fn fetch_histdata(base: &str, pair: &str, p: Period) -> Option<Fetched> {
    let by = source::Declared::default();
    let page =
        source::check_url(&format!("{base}/{}", history::histdata_page_path(pair, p))).ok()?;
    let html = match source::get_found(&page, &by).await {
        Ok(Some(b)) => String::from_utf8_lossy(&b).into_owned(),
        Ok(None) => return Some(Fetched::Empty),
        Err(code) => {
            tracing::info!(%page, code, "HistData page not read");
            return None;
        }
    };
    let Some(fields) = history::histdata_form(&html) else {
        return Some(Fetched::Empty);
    };
    let get = source::check_url(&format!("{base}/get.php")).ok()?;
    let zip = match source::post_form_found(&get, &by, &page, &fields).await {
        // An empty body is a period without a file, as a page without one.
        Ok(Some(z)) if !z.is_empty() => z,
        Ok(_) => return Some(Fetched::Empty),
        Err(code) => {
            tracing::info!(%page, code, "HistData file not read");
            return None;
        }
    };
    let sha256 = hex(&Sha256::digest(&zip));
    let pair = pair.to_string();
    let decoded = tokio::task::spawn_blocking(move || {
        let csv = pnex_core::import_read::unzip_csv(&zip).ok()?;
        let rows = history::histdata_mt_m1(&csv).ok()?;
        let src = HISTDATA_M1;
        ohlcv_batch(
            src.interval,
            src.key,
            &vec![pair.as_str(); rows.len()],
            &rows,
        )
        .ok()
    })
    .await
    .ok()
    .flatten();
    Some(match decoded {
        None => Fetched::Failed("unreadable"),
        Some(b) if b.num_rows() == 0 => Fetched::Empty,
        Some(b) => Fetched::Rows(b, sha256),
    })
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// The rows of a klines CSV, `None` when unreadable.
fn klines_of(csv: &[u8], symbol: &str) -> Option<RecordBatch> {
    let rows = history::binance_klines(csv).ok()?;
    let src = BINANCE_SPOT_1M;
    ohlcv_batch(src.interval, src.key, &vec![symbol; rows.len()], &rows).ok()
}

/// The rows of a trades CSV, `None` when unreadable.
fn trades_of(csv: &[u8], symbol: &str) -> Option<RecordBatch> {
    let rows = history::binance_trades(csv).ok()?;
    trade_tick_batch(&BINANCE_SPOT_TRADES, symbol, &rows).ok()
}

pub(crate) async fn settled(
    db: &DatabaseConnection,
    org: i64,
    src: &HistorySource,
    symbol: &str,
    today: NaiveDate,
) -> Result<(HashSet<Period>, HashSet<Period>), DatasetError> {
    let rows = db
        .query_all_raw(stmt(
            "SELECT period, period_start, state FROM market_history_partitions \
             WHERE org_id = $1 AND source = $2 AND instrument = $3",
            vec![org.into(), src.key.into(), symbol.into()],
        ))
        .await?;
    let (mut done, mut seen) = (HashSet::new(), HashSet::new());
    for r in rows {
        let kind: String = r.try_get("", "period")?;
        let start: NaiveDate = r.try_get("", "period_start")?;
        let state: String = r.try_get("", "state")?;
        if let Some(p) = Period::of(&kind, start) {
            seen.insert(p);
            if p.settled(&state, today) {
                done.insert(p);
            }
        }
    }
    Ok((done, seen))
}

pub(crate) enum Fetched {
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

/// A Binance zip and its `.CHECKSUM`, decoded by `decode`.
async fn fetch_binance(
    base: &str,
    path: &str,
    symbol: &str,
    decode: fn(&[u8], &str) -> Option<RecordBatch>,
) -> Option<Fetched> {
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
    let sha256 = hex(&Sha256::digest(&zip));
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
    let symbol = symbol.to_string();
    let decoded = tokio::task::spawn_blocking(move || {
        let csv = pnex_core::import_read::unzip_csv(&zip).ok()?;
        decode(&csv, &symbol)
    })
    .await
    .ok()
    .flatten();
    Some(match decoded {
        None => Fetched::Failed("unreadable"),
        Some(b) if b.num_rows() == 0 => Fetched::Empty,
        Some(b) => Fetched::Rows(b, sha256),
    })
}

/// Columns of the `ohlcv` shape (D809); `instruments` holds one per row.
pub fn ohlcv_batch(
    interval: &str,
    source: &str,
    instruments: &[&str],
    rows: &[Kline],
) -> Result<RecordBatch, arrow_schema::ArrowError> {
    let scale = history::SCALE as i8;
    let dec = || DataType::Decimal128(38, scale);
    let ts = DataType::Timestamp(TimeUnit::Microsecond, Some(UTC.into()));
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
            Arc::new(StringArray::from(instruments.to_vec())),
            Arc::new(
                TimestampMicrosecondArray::from(rows.iter().map(|k| k.open_us).collect::<Vec<_>>())
                    .with_timezone(UTC),
            ),
            Arc::new(StringArray::from(vec![interval; n])),
            Arc::new(col(|k| k.open)?),
            Arc::new(col(|k| k.high)?),
            Arc::new(col(|k| k.low)?),
            Arc::new(col(|k| k.close)?),
            Arc::new(col(|k| k.volume)?),
            Arc::new(
                Decimal128Array::from(rows.iter().map(|k| k.quote_volume).collect::<Vec<_>>())
                    .with_precision_and_scale(38, scale)?,
            ),
            Arc::new(Int64Array::from(
                rows.iter().map(|k| k.trades).collect::<Vec<_>>(),
            )),
            Arc::new(StringArray::from(vec![source; n])),
        ],
    )
}

/// Ticks per batch of a streamed month: a month holds millions.
const TICK_BATCH: usize = 500_000;
/// A month of ticks unzips to a few hundred MB; past this it is no tick file.
const TICK_FILE_MAX: u64 = 4 << 30;

/// The `.csv` entry of a HistData tick zip, read line by line into batches
/// sent as they fill; `None` is sent once it was read to its end, never on
/// an error (the transaction is then aborted).
fn decode_histdata_ticks(
    zip: &[u8],
    pair: &str,
    out: &tokio::sync::mpsc::Sender<Option<RecordBatch>>,
) -> Result<(), &'static str> {
    use std::io::{BufRead, BufReader, Read};
    const BAD: &str = "unreadable";
    let mut archive = zip::ZipArchive::new(std::io::Cursor::new(zip)).map_err(|_| BAD)?;
    let index = (0..archive.len())
        .find(|i| {
            archive.by_index(*i).is_ok_and(|f| {
                f.name()
                    .is_ok_and(|n| n.to_ascii_lowercase().ends_with(".csv"))
            })
        })
        .ok_or(BAD)?;
    let file = archive.by_index(index).map_err(|_| BAD)?;
    let reader = BufReader::with_capacity(1 << 20, file.take(TICK_FILE_MAX));
    let send = |rows: &[Tick]| {
        let batch =
            quote_tick_batch(HISTDATA_TICKS.key, &vec![pair; rows.len()], rows).map_err(|_| BAD)?;
        out.blocking_send(Some(batch)).map_err(|_| BAD)
    };
    let mut rows = Vec::with_capacity(TICK_BATCH);
    for line in reader.lines() {
        let line = line.map_err(|_| BAD)?;
        if line.trim().is_empty() {
            continue;
        }
        rows.push(history::histdata_tick_line(&line).ok_or(BAD)?);
        if rows.len() == TICK_BATCH {
            send(&rows)?;
            rows.clear();
        }
    }
    if !rows.is_empty() {
        send(&rows)?;
    }
    out.blocking_send(None).map_err(|_| BAD)
}

/// A HistData month of ticks: the page, its form sent back with the page as
/// referer, then the zip streamed into the table as one transaction. A page
/// without a file, or a file without a tick, is `Empty`.
async fn fetch_histdata_ticks(
    db: &DatabaseConnection,
    store: &S3Store,
    org: i64,
    base: &str,
    pair: &str,
    p: Period,
) -> Option<Fetched> {
    let by = source::Declared::default();
    let page = source::check_url(&format!(
        "{base}/{}",
        history::histdata_tick_page_path(pair, p.start())
    ))
    .ok()?;
    let html = match source::get_found(&page, &by).await {
        Ok(Some(b)) => String::from_utf8_lossy(&b).into_owned(),
        Ok(None) => return Some(Fetched::Empty),
        Err(code) => {
            tracing::info!(%page, code, "HistData tick page not read");
            return None;
        }
    };
    let Some(fields) = history::histdata_form(&html) else {
        return Some(Fetched::Empty);
    };
    let get = source::check_url(&format!("{base}/get.php")).ok()?;
    let zip = match source::post_form_found(&get, &by, &page, &fields).await {
        Ok(Some(z)) if !z.is_empty() => z,
        Ok(_) => return Some(Fetched::Empty),
        Err(code) => {
            tracing::info!(%page, code, "HistData tick file not read");
            return None;
        }
    };
    let sha256 = hex(&Sha256::digest(&zip));
    let (tx, rx) = tokio::sync::mpsc::channel::<Option<RecordBatch>>(2);
    let owned = pair.to_string();
    let decode = tokio::task::spawn_blocking(move || decode_histdata_ticks(&zip, &owned, &tx));
    let appended = async {
        let table = table_of(db, org, &HISTDATA_TICKS).await?;
        datasets::append_stream(db, store, org, table, rx).await
    }
    .await;
    let decoded = decode.await.unwrap_or(Err("unreadable"));
    Some(match (appended, decoded) {
        (Ok(Some(t)), _) => Fetched::Done {
            transaction: t.id.parse().unwrap_or_default(),
            rows: t.rows,
            bytes: t.bytes,
            sha256,
        },
        (Ok(None), _) => Fetched::Empty,
        (Err(DatasetError::QuotaExceeded), _) => Fetched::Failed("quota"),
        (Err(DatasetError::SchemaMismatch { .. }), _) => Fetched::Failed("schema"),
        (Err(_), Err(code)) => Fetched::Failed(code),
        (Err(e), Ok(())) => {
            tracing::warn!(org, code = e.code(), "HistData ticks not written");
            Fetched::Failed("storage")
        }
    })
}

/// Columns of the `quote_tick` shape (D809); `instruments` holds one per row.
pub fn quote_tick_batch(
    source: &str,
    instruments: &[&str],
    rows: &[Tick],
) -> Result<RecordBatch, arrow_schema::ArrowError> {
    let scale = history::SCALE as i8;
    let dec = || DataType::Decimal128(38, scale);
    let ts = DataType::Timestamp(TimeUnit::Microsecond, Some(UTC.into()));
    let schema = Arc::new(Schema::new(vec![
        Field::new("instrument", DataType::Utf8, false),
        Field::new("ts", ts, false),
        Field::new("bid", dec(), false),
        Field::new("ask", dec(), false),
        Field::new("bid_volume", DataType::Float64, true),
        Field::new("ask_volume", DataType::Float64, true),
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
            Arc::new(StringArray::from(instruments.to_vec())),
            Arc::new(
                TimestampMicrosecondArray::from(rows.iter().map(|t| t.ts_us).collect::<Vec<_>>())
                    .with_timezone(UTC),
            ),
            Arc::new(col(|t| t.bid)?),
            Arc::new(col(|t| t.ask)?),
            Arc::new(Float64Array::from(
                rows.iter().map(|t| t.bid_volume).collect::<Vec<_>>(),
            )),
            Arc::new(Float64Array::from(
                rows.iter().map(|t| t.ask_volume).collect::<Vec<_>>(),
            )),
            Arc::new(StringArray::from(vec![source; n])),
        ],
    )
}

/// Columns of the `trade_tick` shape (D809).
pub fn trade_tick_batch(
    src: &HistorySource,
    instrument: &str,
    rows: &[Trade],
) -> Result<RecordBatch, arrow_schema::ArrowError> {
    let scale = history::SCALE as i8;
    let dec = || DataType::Decimal128(38, scale);
    let ts = DataType::Timestamp(TimeUnit::Microsecond, Some(UTC.into()));
    let schema = Arc::new(Schema::new(vec![
        Field::new("instrument", DataType::Utf8, false),
        Field::new("ts", ts, false),
        Field::new("price", dec(), false),
        Field::new("qty", dec(), false),
        Field::new("side", DataType::Utf8, true),
        Field::new("trade_id", DataType::Int64, false),
        Field::new("source", DataType::Utf8, false),
    ]));
    let n = rows.len();
    let col = |f: fn(&Trade) -> i128| {
        Decimal128Array::from(rows.iter().map(f).collect::<Vec<_>>())
            .with_precision_and_scale(38, scale)
    };
    RecordBatch::try_new(
        schema,
        vec![
            Arc::new(StringArray::from(vec![instrument; n])),
            Arc::new(
                TimestampMicrosecondArray::from(rows.iter().map(|t| t.ts_us).collect::<Vec<_>>())
                    .with_timezone(UTC),
            ),
            Arc::new(col(|t| t.price)?),
            Arc::new(col(|t| t.qty)?),
            Arc::new(StringArray::from(
                rows.iter().map(|t| Some(t.side)).collect::<Vec<_>>(),
            )),
            Arc::new(Int64Array::from(
                rows.iter().map(|t| t.trade_id).collect::<Vec<_>>(),
            )),
            Arc::new(StringArray::from(vec![src.key; n])),
        ],
    )
}

/// The org's table of this source, created the first time.
pub(crate) async fn table_of(
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
                other => format!("{other:?}"),
            };
            tracing::warn!(org, table = src.table, code = e.code(), %detail, "price history not written");
            Fetched::Failed("storage")
        }
    }
}

pub(crate) async fn record(
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
        let today = NaiveDate::from_ymd_opt(2026, 11, 8).unwrap();
        let d = |y, m, d| NaiveDate::from_ymd_opt(y, m, d).unwrap();
        assert_eq!(
            Job::Klines.instruments(" btcusdt, ETHUSDT ,../x,,BTCUSDT"),
            vec!["BTCUSDT", "ETHUSDT"]
        );
        assert_eq!(
            Job::Ticks.instruments("eurusd, XAUUSD,usdjpy,EURUSD"),
            vec!["EURUSD", "USDJPY"]
        );
        assert_eq!(Job::Klines.from("", today), d(2017, 8, 1));
        assert_eq!(Job::Klines.from("junk", today), d(2017, 8, 1));
        assert_eq!(Job::Klines.from("2024-03", today), d(2024, 3, 1));
        assert_eq!(Job::Ticks.from("", today), d(2025, 11, 8));
        assert_eq!(Job::Ticks.from("2025-03", today), d(2025, 3, 1));
        assert_eq!(Job::Trades.from("", today), d(2026, 10, 8));
        assert_eq!(Job::HistData.from("", today), d(2000, 1, 1));
        assert_eq!(Job::HistData.from("2010", today), d(2010, 1, 1));
        assert_eq!(
            Job::HistData.instruments("eurusd, XAUUSD,spxusd,eur/usd"),
            vec!["EURUSD", "XAUUSD", "SPXUSD"]
        );
    }
}
