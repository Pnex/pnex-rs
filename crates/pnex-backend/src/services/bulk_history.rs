//! A day of ADS-B aircraft positions (adsb.lol, markets-research.md MK24)
//! into a table. AIS and DVF are pipelines of their packs now
//! (pipeline-engine.md); ADS-B waits for a step that streams a file split in
//! parts of 2 GB without keeping it. A file of several GB is never held: it is read as it
//! arrives, decoded on a blocking thread and written batch by batch into
//! one `append` transaction of the source's table, committed only once the
//! file was read to its end. Partitions are kept in the registry of the
//! price history (`market_history_partitions`), so a cut run goes on where
//! it stopped. Everything stays under the org's dataset quota (Q-F8).

use std::io::Read;
use std::sync::Arc;
use std::time::Duration;

use arrow_array::{
    ArrayRef, BooleanArray, Float64Array, RecordBatch, StringArray, TimestampMicrosecondArray,
};
use arrow_schema::{DataType, Field, Schema, TimeUnit};
use chrono::NaiveDate;
use loco_rs::app::AppContext;
use pnex_core::bulk::{self, BulkSource, FlightPoint};
use pnex_core::markets::history::{self as hist, HistorySource, Period};
use sea_orm::{ConnectionTrait, DatabaseBackend, DatabaseConnection, Statement};
use sha2::{Digest, Sha256};
use tokio::sync::mpsc;

use crate::services::artifact_store::S3Store;
use crate::services::datasets::{self, DatasetError};
use crate::services::db_lock::{ns, TenantLock};
use crate::services::markets::history::{record, settled, table_of, Fetched, UTC};
use crate::services::ontology::imports::source;

/// Rows per written batch: a few tens of MB in memory at most.
const BATCH_ROWS: usize = 200_000;
const LOCK_WAIT: Duration = Duration::from_secs(1);
const DECODE_ERROR: &str = "unreadable";
/// Largest trace of one aircraft for a day, packed or not (a few MB at most).
const TRACE_MAX: u64 = 64 * 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Job {
    Adsb,
}

const JOBS: [Job; 1] = [Job::Adsb];

impl Job {
    fn src(self) -> BulkSource {
        match self {
            Self::Adsb => bulk::ADSB_LOL,
        }
    }

    /// The setting of the first partition; empty: the source is not read.
    fn setting(self) -> &'static str {
        match self {
            Self::Adsb => "trade_flows.adsb_from",
        }
    }

    fn contact_setting(self) -> &'static str {
        match self {
            Self::Adsb => "trade_flows.source_contact",
        }
    }

    /// `YYYY-MM-DD` for a daily source, `YYYY` for DVF.
    fn from(self, raw: &str) -> Option<NaiveDate> {
        let raw = raw.trim();
        match self {
            Self::Adsb => NaiveDate::parse_from_str(raw, "%Y-%m-%d").ok(),
        }
    }

    fn plan(
        self,
        from: NaiveDate,
        today: NaiveDate,
        known: &std::collections::HashSet<Period>,
    ) -> Vec<Period> {
        match self {
            Self::Adsb => hist::plan_days(from, today, known),
        }
    }

    /// The file of a partition; several parts (adsb.lol) are one stream.
    fn urls(self, p: Period) -> Vec<String> {
        let day = p.start();
        match self {
            Self::Adsb => (0..26)
                .filter_map(|i| bulk::adsb_part_url(day, i))
                .collect(),
        }
    }
}

/// The registry and the tables take a price history source: same key and
/// table, nothing else read.
fn registry_src(b: BulkSource) -> HistorySource {
    HistorySource {
        key: b.key,
        base: "",
        interval: "",
        table: b.table,
        license_note: b.license_note,
    }
}

fn stmt(sql: &str, values: Vec<sea_orm::Value>) -> Statement {
    Statement::from_sql_and_values(DatabaseBackend::Postgres, sql, values)
}

/// One pass over the orgs that list a first partition.
pub async fn sweep(ctx: &AppContext, today: NaiveDate) -> Result<(), DatasetError> {
    sweep_with(ctx, today, None).await
}

/// [`sweep`] with every file read from `mock` (same path), for tests.
pub async fn sweep_with(
    ctx: &AppContext,
    today: NaiveDate,
    mock: Option<&str>,
) -> Result<(), DatasetError> {
    let orgs: Vec<i64> = ctx
        .db
        .query_all_raw(stmt(
            "SELECT DISTINCT org_id FROM packs WHERE key = 'trade_flows' \
             ORDER BY org_id",
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
        // Every member owns an org of their own: several GB a day per org is
        // the platform's bandwidth and disk, so only an org whose dataset
        // quota a platform administrator set explicitly loads them (SEC-82).
        if datasets::quota_override(&ctx.db, org).await?.is_none() {
            continue;
        }
        // Shared with the price history: one heavy load per org at a time.
        let Ok(lock) = TenantLock::acquire(&ctx.db, ns::MARKET_HISTORY, org, LOCK_WAIT).await
        else {
            continue;
        };
        for job in JOBS {
            if let Err(e) = pass(&ctx.db, &store, org, today, job, mock).await {
                tracing::warn!(org, ?job, code = e.code(), "bulk history pass stopped");
            }
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
        crate::services::ontology::packs::setting_value(db, org, key)
            .await?
            .and_then(|v| v.as_str().map(str::to_string))
            .unwrap_or_default(),
    )
}

fn rebase(url: &str, mock: Option<&str>) -> String {
    match (mock, reqwest::Url::parse(url)) {
        (Some(base), Ok(u)) => format!("{base}{}", u.path()),
        _ => url.to_string(),
    }
}

/// One partition of a job per pass: a file can take minutes.
async fn pass(
    db: &DatabaseConnection,
    store: &S3Store,
    org: i64,
    today: NaiveDate,
    job: Job,
    mock: Option<&str>,
) -> Result<(), DatasetError> {
    let Some(from) = job.from(&text_setting(db, org, job.setting()).await?) else {
        return Ok(());
    };
    let src = registry_src(job.src());
    let instrument = src.key;
    let (known, seen) = settled(db, org, &src, instrument, today).await?;
    let (fresh, again): (Vec<Period>, Vec<Period>) = job
        .plan(from, today, &known)
        .into_iter()
        .partition(|p| !seen.contains(p));
    let next = fresh.into_iter().chain(again).next();
    let Some(p) = next else {
        return Ok(());
    };
    let contact =
        Some(text_setting(db, org, job.contact_setting()).await?).filter(|c| !c.trim().is_empty());
    let urls = job.urls(p).iter().map(|u| rebase(u, mock)).collect();
    let Some(outcome) = load(db, store, org, &src, job, urls, contact).await else {
        return Ok(());
    };
    let quota = matches!(&outcome, Fetched::Failed("quota"));
    record(db, org, &src, instrument, p, &outcome).await?;
    if quota {
        return Err(DatasetError::QuotaExceeded);
    }
    Ok(())
}

/// Reads the body chunks a pump sends; an error of the pump is a read error.
struct ChanReader {
    rx: mpsc::Receiver<Result<Vec<u8>, ()>>,
    buf: Vec<u8>,
    pos: usize,
}

impl Read for ChanReader {
    fn read(&mut self, out: &mut [u8]) -> std::io::Result<usize> {
        while self.pos == self.buf.len() {
            match self.rx.blocking_recv() {
                Some(Ok(b)) => (self.buf, self.pos) = (b, 0),
                Some(Err(())) => return Err(std::io::Error::other("download cut")),
                None => return Ok(0),
            }
        }
        let n = out.len().min(self.buf.len() - self.pos);
        out[..n].copy_from_slice(&self.buf[self.pos..self.pos + n]);
        self.pos += n;
        Ok(n)
    }
}

/// Downloads, decodes and writes one partition. `None`: the source did not
/// answer or the download was cut, nothing is recorded and it is asked
/// again later.
async fn load(
    db: &DatabaseConnection,
    store: &S3Store,
    org: i64,
    src: &HistorySource,
    job: Job,
    urls: Vec<String>,
    contact: Option<String>,
) -> Option<Fetched> {
    let declared = |contact: &Option<String>| source::Declared {
        contact: contact.clone(),
        ..Default::default()
    };
    let first = source::check_url(urls.first()?).ok()?;
    let resp = match source::open_large(&first, &declared(&contact)).await {
        Ok(Some(r)) => r,
        Ok(None) => return Some(Fetched::Empty),
        Err(code) => {
            tracing::info!(%first, code, "bulk file not read");
            return None;
        }
    };
    let (bytes_tx, bytes_rx) = mpsc::channel::<Result<Vec<u8>, ()>>(64);
    // The pump: the parts in turn into one stream, hashed as they pass.
    let pump = tokio::spawn(async move {
        let mut digest = Sha256::new();
        let mut current = Some(resp);
        let mut rest = urls.into_iter().skip(1);
        while let Some(mut r) = current.take() {
            loop {
                match r.chunk().await {
                    Ok(Some(c)) => {
                        digest.update(&c);
                        if bytes_tx.send(Ok(c.to_vec())).await.is_err() {
                            return Err(());
                        }
                    }
                    Ok(None) => break,
                    Err(_) => {
                        let _ = bytes_tx.send(Err(())).await;
                        return Err(());
                    }
                }
            }
            let Some(next) = rest.next() else { break };
            let Ok(u) = source::check_url(&next) else {
                break;
            };
            match source::open_large(&u, &declared(&contact)).await {
                Ok(Some(r)) => current = Some(r),
                // The previous part was the last.
                Ok(None) => break,
                Err(_) => {
                    let _ = bytes_tx.send(Err(())).await;
                    return Err(());
                }
            }
        }
        Ok(digest
            .finalize()
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect::<String>())
    });
    let (batch_tx, batch_rx) = mpsc::channel::<Option<RecordBatch>>(2);
    let reader = ChanReader {
        rx: bytes_rx,
        buf: Vec::new(),
        pos: 0,
    };
    let decode = tokio::task::spawn_blocking(move || decode(job, reader, &batch_tx));
    let appended = async {
        let table = table_of(db, org, src).await?;
        datasets::append_stream(db, store, org, table, batch_rx).await
    }
    .await;
    let decoded = decode.await.unwrap_or(Err(DECODE_ERROR));
    let pumped = pump.await.unwrap_or(Err(()));
    Some(match appended {
        Ok(Some(t)) => Fetched::Done {
            transaction: t.id.parse().unwrap_or_default(),
            rows: t.rows,
            bytes: t.bytes,
            sha256: pumped.unwrap_or_default(),
        },
        Ok(None) => Fetched::Empty,
        Err(DatasetError::QuotaExceeded) => Fetched::Failed("quota"),
        Err(DatasetError::SchemaMismatch { .. }) => Fetched::Failed("schema"),
        Err(e) => {
            if pumped.is_err() && decoded.is_ok() {
                // A cut download, not a bad file: asked again later.
                return None;
            }
            match decoded {
                Err(code) => Fetched::Failed(code),
                Ok(()) => {
                    tracing::warn!(
                        org,
                        table = src.table,
                        code = e.code(),
                        "bulk file not written"
                    );
                    Fetched::Failed("storage")
                }
            }
        }
    })
}

fn send(out: &mpsc::Sender<Option<RecordBatch>>, b: RecordBatch) -> Result<(), &'static str> {
    out.blocking_send(Some(b)).map_err(|_| "closed")
}

/// Reads what is left so the pump ends cleanly (a zip's directory).
fn drain(mut r: impl Read) -> Result<(), &'static str> {
    std::io::copy(&mut r, &mut std::io::sink())
        .map(|_| ())
        .map_err(|_| DECODE_ERROR)
}

/// Decodes a whole partition into batches; `None` is sent once it was read
/// to its end, never on an error (the transaction is then aborted).
fn decode(
    job: Job,
    mut reader: ChanReader,
    out: &mpsc::Sender<Option<RecordBatch>>,
) -> Result<(), &'static str> {
    match job {
        Job::Adsb => {
            let mut archive = tar::Archive::new(&mut reader);
            let mut points: Vec<FlightPoint> = Vec::with_capacity(BATCH_ROWS);
            for entry in archive.entries().map_err(|_| DECODE_ERROR)? {
                let mut e = entry.map_err(|_| DECODE_ERROR)?;
                let is_trace = e
                    .path()
                    .ok()
                    .and_then(|p| p.file_name().map(|n| n.to_string_lossy().into_owned()))
                    .is_some_and(|n| n.starts_with("trace_full_"));
                // Not a trace, or not one aircraft's day: skipped unread.
                if !is_trace || e.size() > TRACE_MAX {
                    continue;
                }
                let mut raw = Vec::new();
                e.read_to_end(&mut raw).map_err(|_| DECODE_ERROR)?;
                let json = if raw.starts_with(&[0x1f, 0x8b]) {
                    let mut plain = Vec::new();
                    // A packed trace never unpacks past TRACE_MAX.
                    flate2::read::GzDecoder::new(&raw[..])
                        .take(TRACE_MAX)
                        .read_to_end(&mut plain)
                        .map_err(|_| DECODE_ERROR)?;
                    plain
                } else {
                    raw
                };
                // One unreadable trace is one aircraft lost, not the day.
                if let Some(p) = bulk::adsb_trace(&json) {
                    points.extend(p);
                }
                if points.len() >= BATCH_ROWS {
                    send(out, flights_batch(&points)?)?;
                    points.clear();
                }
            }
            if !points.is_empty() {
                send(out, flights_batch(&points)?)?;
            }
            drain(&mut reader)?;
        }
    }
    out.blocking_send(None).map_err(|_| "closed")
}

fn ts_field() -> Field {
    Field::new(
        "ts",
        DataType::Timestamp(TimeUnit::Microsecond, Some(UTC.into())),
        false,
    )
}

fn f64_col<'a, T: 'a>(rows: &'a [T], f: impl Fn(&T) -> Option<f64>) -> ArrayRef {
    Arc::new(rows.iter().map(f).collect::<Float64Array>())
}

fn str_col<'a, T: 'a>(rows: &'a [T], f: impl Fn(&T) -> Option<&str>) -> ArrayRef {
    Arc::new(rows.iter().map(f).collect::<StringArray>())
}

fn batch(fields: Vec<Field>, cols: Vec<ArrayRef>) -> Result<RecordBatch, &'static str> {
    RecordBatch::try_new(Arc::new(Schema::new(fields)), cols).map_err(|_| DECODE_ERROR)
}

fn flights_batch(rows: &[FlightPoint]) -> Result<RecordBatch, &'static str> {
    let f = |n: &str, t: DataType| Field::new(n, t, true);
    batch(
        vec![
            ts_field(),
            Field::new("icao", DataType::Utf8, false),
            f("registration", DataType::Utf8),
            f("aircraft_type", DataType::Utf8),
            Field::new("lat", DataType::Float64, false),
            Field::new("lon", DataType::Float64, false),
            f("alt_baro_ft", DataType::Float64),
            Field::new("on_ground", DataType::Boolean, false),
            f("ground_speed", DataType::Float64),
            f("track", DataType::Float64),
            f("vertical_rate", DataType::Float64),
            f("source", DataType::Utf8),
        ],
        vec![
            Arc::new(
                TimestampMicrosecondArray::from(rows.iter().map(|r| r.ts_us).collect::<Vec<_>>())
                    .with_timezone(UTC),
            ),
            str_col(rows, |r| Some(r.icao.as_str())),
            str_col(rows, |r| r.registration.as_deref()),
            str_col(rows, |r| r.aircraft_type.as_deref()),
            f64_col(rows, |r| Some(r.lat)),
            f64_col(rows, |r| Some(r.lon)),
            f64_col(rows, |r| r.alt_baro_ft),
            Arc::new(BooleanArray::from(
                rows.iter().map(|r| r.on_ground).collect::<Vec<_>>(),
            )),
            f64_col(rows, |r| r.ground_speed),
            f64_col(rows, |r| r.track),
            f64_col(rows, |r| r.vertical_rate),
            str_col(rows, |r| r.source.as_deref()),
        ],
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings_and_files_of_each_job() {
        assert_eq!(
            Job::Adsb.from("2026-11-01"),
            NaiveDate::from_ymd_opt(2026, 11, 1)
        );
        assert_eq!(Job::Adsb.from(""), None, "empty: not read");
        let day = Period::Day(NaiveDate::from_ymd_opt(2025, 12, 30).unwrap());
        assert_eq!(Job::Adsb.urls(day).len(), 26);
        assert_eq!(
            rebase("https://example.org/a/b.tar.aa", Some("http://127.0.0.1:9")),
            "http://127.0.0.1:9/a/b.tar.aa"
        );
    }
}
