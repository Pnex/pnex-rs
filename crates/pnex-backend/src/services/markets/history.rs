//! Price rows pushed by MetaTrader 5 terminals (markets-research.md D849).
//! The public price and bulk files are pipelines (pipeline-engine.md D939).

use std::sync::Arc;

use arrow_array::{
    Decimal128Array, Float64Array, Int64Array, RecordBatch, StringArray, TimestampMicrosecondArray,
};
use arrow_schema::{DataType, Field, Schema, TimeUnit};
use chrono::NaiveDate;
use loco_rs::app::AppContext;
use pnex_core::markets::history::{self, HistorySource, Kline, Tick, MT5_OHLCV_1M, MT5_TICKS};
use sea_orm::{ConnectionTrait, DatabaseBackend, DatabaseConnection, Statement};
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
        for call in &calls {
            let body: serde_json::Value = call.try_get("", "body")?;
            match history::mt5_payload(&body) {
                Ok(b) => {
                    bars.entry(b.source.clone()).or_default().extend(b.bars);
                    ticks.entry(b.source).or_default().extend(b.ticks);
                }
                Err(token) => tracing::warn!(org, token, "MT5 terminal call dropped"),
            }
        }
        for (source, rows) in bars.into_iter().filter(|(_, r)| !r.is_empty()) {
            let names: Vec<&str> = rows.iter().map(|(s, _)| s.as_str()).collect();
            let klines: Vec<Kline> = rows.iter().map(|(_, k)| k.clone()).collect();
            let batch = ohlcv_batch(MT5_OHLCV_1M.interval, &source, &names, &klines)
                .map_err(|e| DatasetError::Storage(e.to_string()))?;
            write(db, store, org, &MT5_OHLCV_1M, batch).await?;
        }
        for (source, rows) in ticks.into_iter().filter(|(_, r)| !r.is_empty()) {
            let names: Vec<&str> = rows.iter().map(|(s, _)| s.as_str()).collect();
            let quotes: Vec<Tick> = rows.iter().map(|(_, t)| t.clone()).collect();
            let batch = quote_tick_batch(&source, &names, &quotes)
                .map_err(|e| DatasetError::Storage(e.to_string()))?;
            write(db, store, org, &MT5_TICKS, batch).await?;
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

/// Appends a batch; a refusal stops the pass and keeps the calls.
async fn write(
    db: &DatabaseConnection,
    store: &S3Store,
    org: i64,
    src: &HistorySource,
    batch: RecordBatch,
) -> Result<(), DatasetError> {
    let out = async {
        let table = table_of(db, org, src).await?;
        datasets::append_batch(db, store, org, table, &batch).await
    }
    .await;
    match out {
        Ok(_) => Ok(()),
        Err(e @ DatasetError::QuotaExceeded) => Err(e),
        Err(e) => {
            let detail = match &e {
                DatasetError::Storage(m) => m.clone(),
                other => format!("{other:?}"),
            };
            tracing::warn!(org, table = src.table, code = e.code(), %detail, "price history not written");
            Err(DatasetError::Storage("MT5 rows not written".into()))
        }
    }
}
