//! Price rows pushed by MetaTrader 5 terminals (markets-research.md D849),
//! and the registry helpers the large public files still use (D808). The
//! public price files themselves are pipelines (pipeline-engine.md D939).

use std::collections::HashSet;
use std::sync::Arc;

use arrow_array::{
    Decimal128Array, Float64Array, Int64Array, RecordBatch, StringArray, TimestampMicrosecondArray,
};
use arrow_schema::{DataType, Field, Schema, TimeUnit};
use chrono::NaiveDate;
use loco_rs::app::AppContext;
use pnex_core::markets::history::{
    self, HistorySource, Kline, Period, Tick, MT5_OHLCV_1M, MT5_TICKS,
};
use sea_orm::{ConnectionTrait, DatabaseBackend, DatabaseConnection, Statement};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::services::artifact_store::S3Store;
use crate::services::datasets::{self, DatasetError};
use crate::services::db_lock::{ns, TenantLock};

const LOCK_WAIT: std::time::Duration = std::time::Duration::from_secs(1);

pub(crate) const UTC: &str = "+00:00";

fn stmt(sql: &str, values: Vec<sea_orm::Value>) -> Statement {
    Statement::from_sql_and_values(DatabaseBackend::Postgres, sql, values)
}

/// One pass of the MT5 terminals' inbox of each org that has calls in it
/// (D849). The public price files are pipelines now (pipeline-engine.md).
pub async fn sweep(ctx: &AppContext, _today: NaiveDate) -> Result<(), DatasetError> {
    let orgs: Vec<i64> = ctx
        .db
        .query_all_raw(stmt(
            "SELECT DISTINCT org_id FROM mt5_inbox ORDER BY org_id",
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
        // A pass outliving its sweep turn must not run beside the next one.
        let Ok(lock) = TenantLock::acquire(&ctx.db, ns::MARKET_HISTORY, org, LOCK_WAIT).await
        else {
            tracing::info!(org, "MT5 terminals pass already running");
            continue;
        };
        if let Err(e) = mt5_pass(&ctx.db, &store, org).await {
            tracing::warn!(org, code = e.code(), "MT5 terminals pass stopped");
        }
        lock.release().await;
    }
    Ok(())
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

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// The rows of a klines CSV, `None` when unreadable.
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
    Empty,
    Failed(&'static str),
    Done {
        transaction: Uuid,
        rows: i64,
        bytes: i64,
        sha256: String,
    },
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
