//! Datasets (data-pipelines.md D569): tables as Parquet files on S3, versioned
//! by transactions, with the catalogue in PostgreSQL. The current view is the
//! latest committed snapshot, then the appends committed after it.

pub mod csv;
pub mod erasure;
pub mod parquet;
pub mod uploads;

use std::time::{Duration, Instant};

use ::parquet::arrow::async_writer::AsyncArrowWriter;
use ::parquet::basic::{Compression, ZstdLevel};
use ::parquet::file::properties::WriterProperties;
use bytes::Bytes;
use futures_util::{Stream, StreamExt};
use pnex_core::datasets::{
    Dataset, DatasetColumn, DatasetSample, DatasetTransaction, DatasetTransactionKind,
    DatasetTransactionState, NAME_MAX, SAMPLE_ROWS_MAX,
};
use pnex_core::err_codes;
use sea_orm::{
    ConnectionTrait, DatabaseBackend, DatabaseConnection, DbErr, QueryResult, Statement,
    TransactionTrait,
};
use uuid::Uuid;

use self::csv::{CsvBatches, CsvError};
use self::parquet::S3Sink;
use crate::services::artifact_store::{S3Config, S3Store};

/// Parquet bytes an org holds in its datasets, unless a platform admin set
/// another value for it (D821).
// shortcut: a per-org platform setting, move it to the tier (D569) when tiers sell storage.
pub const QUOTA_BYTES: u64 = 20 * 1024 * 1024 * 1024;
/// CSV bytes of one load.
pub use pnex_core::datasets::SOURCE_MAX_BYTES;
/// A transaction with no heartbeat for this long is dead (same as D345).
const DEAD_AFTER_MINUTES: i64 = 10;
const HEARTBEAT_EVERY: Duration = Duration::from_secs(60);
/// A row group past this size goes to the store, whatever its row count.
const ROW_GROUP_BYTES: usize = 64 * 1024 * 1024;
/// How long a replaced version stays readable through `as_of`, unless the transform
/// writing the dataset sets its own (`datasets.retain_days`).
const RETAIN_DAYS: u16 = pnex_core::pipelines::RETAIN_DAYS_DEFAULT;
const PURGE_EVERY: Duration = Duration::from_secs(600);
const PURGE_BATCH: i64 = 500;
/// No upload lasts this long, media or table: one this old lost its process.
const UPLOAD_ABANDONED_AFTER: Duration = Duration::from_secs(24 * 3600);

#[derive(Debug)]
pub enum DatasetError {
    Invalid {
        field: String,
        token: String,
    },
    NotFound,
    NameTaken,
    /// Another load of this dataset is running.
    TransactionOpen,
    /// The load lost its heartbeat before it could commit.
    TransactionAborted,
    Csv(CsvError),
    /// An append whose columns are not those of the dataset.
    SchemaMismatch {
        columns: Vec<String>,
    },
    QuotaExceeded,
    SourceTooLarge,
    /// The body stopped before its end.
    SourceInterrupted,
    /// The version asked for has left its retention.
    VersionPurged,
    /// The table of an import: its steps alone load it.
    Reserved,
    UploadNotFound,
    /// The upload no longer takes parts.
    UploadClosed,
    PartTooLarge,
    PartsMissing {
        missing: Vec<i32>,
        count: i32,
    },
    TooManyUploads,
    /// Its conversion killed its server too many times.
    UploadLost,
    /// Deleting it would lose the erasure: its rows could come back.
    ErasurePending,
    Storage(String),
    Db(DbErr),
}

impl From<DbErr> for DatasetError {
    fn from(e: DbErr) -> Self {
        Self::Db(e)
    }
}

impl DatasetError {
    /// Machine code journaled on an aborted transaction.
    pub fn code(&self) -> &'static str {
        match self {
            Self::Csv(_) => err_codes::DATASET_CSV_INVALID,
            Self::SchemaMismatch { .. } => err_codes::DATASET_SCHEMA_MISMATCH,
            Self::QuotaExceeded => err_codes::DATASET_QUOTA,
            Self::SourceTooLarge => err_codes::DATASET_SOURCE_TOO_LARGE,
            Self::SourceInterrupted => err_codes::DATASET_SOURCE_INTERRUPTED,
            Self::Reserved => err_codes::DATASET_RESERVED,
            Self::UploadLost => err_codes::DATASET_UPLOAD_LOST,
            _ => err_codes::DATASET_STORAGE_UNAVAILABLE,
        }
    }

    /// Arguments of the error message, for an answer or a journaled failure.
    pub fn args(&self) -> Option<serde_json::Value> {
        use serde_json::json;
        Some(match self {
            Self::Csv(CsvError::Empty) => json!({ "reason": "empty" }),
            Self::Csv(CsvError::RecordTooLarge { line }) => {
                json!({ "reason": "record_too_large", "line": line })
            }
            Self::Csv(CsvError::TooManyColumns { line }) => {
                json!({ "reason": "too_many_columns", "line": line })
            }
            Self::Csv(CsvError::ColumnNameTooLong) => json!({ "reason": "column_name_too_long" }),
            Self::Csv(CsvError::DuplicateColumn { name }) => {
                json!({ "reason": "duplicate_column", "column": name })
            }
            Self::Csv(CsvError::FieldCount {
                line,
                expected,
                got,
            }) => {
                json!({ "reason": "field_count", "line": line, "expected": expected, "got": got })
            }
            Self::SchemaMismatch { columns } => json!({ "columns": columns }),
            Self::SourceTooLarge => json!({ "max": SOURCE_MAX_BYTES }),
            _ => return None,
        })
    }
}

fn stmt(sql: &str, values: Vec<sea_orm::Value>) -> Statement {
    Statement::from_sql_and_values(DatabaseBackend::Postgres, sql, values)
}

fn storage<E: std::fmt::Display>(e: E) -> DatasetError {
    DatasetError::Storage(e.to_string())
}

/// The object store, or why not.
pub fn store_of(config: &loco_rs::config::Config) -> Result<S3Store, DatasetError> {
    S3Config::from_config(config).store().map_err(storage)
}

/// Prefix of everything a dataset wrote.
fn dataset_prefix(org_id: i64, dataset_id: Uuid) -> String {
    format!("org_{org_id}/datasets/{dataset_id}/")
}

/// Prefix of everything a transaction wrote: an aborted one is removed whole,
/// even a file it wrote and never recorded.
fn transaction_prefix(org_id: i64, dataset_id: Uuid, transaction_id: Uuid) -> String {
    format!("{}{transaction_id}/", dataset_prefix(org_id, dataset_id))
}

/// Transactions in the view of `d`. `AS_OF` and `AS_OF_S` are replaced by a
/// condition on `committed_at`, empty for the current view.
const VIEW_TRANSACTIONS: &str = "\
    t.dataset_id = d.id AND t.state = 'committed' AS_OF AND t.seq >= coalesce(( \
       SELECT max(s.seq) FROM dataset_transactions s \
       WHERE s.dataset_id = d.id AND s.state = 'committed' AND s.kind = 'snapshot' AS_OF_S \
    ), 0)";

const DATASET_COLUMNS: &str = "\
    (SELECT i.name FROM object_imports i WHERE i.id = d.import_id) AS import_name, \
    d.id, d.name, d.schema, d.max_age_hours, \
    (d.max_age_hours IS NOT NULL AND coalesce(v.committed_at, d.created_at) \
      < now() - make_interval(hours => d.max_age_hours)) AS stale, \
    to_char(d.created_at AT TIME ZONE 'UTC', 'YYYY-MM-DD\"T\"HH24:MI:SS.MS\"Z\"') AS created_at, \
    coalesce(v.rows, 0)::bigint AS rows, coalesce(v.bytes, 0)::bigint AS bytes, \
    to_char(v.committed_at AT TIME ZONE 'UTC', 'YYYY-MM-DD\"T\"HH24:MI:SS.US\"Z\"') AS committed_at, \
    coalesce((SELECT jsonb_agg(jsonb_build_object('column', i.column_name, 'type_key', i.type_key, \
      'property', i.property) ORDER BY i.column_name, i.type_key) \
      FROM dataset_identity_columns i WHERE i.dataset_id = d.id), '[]'::jsonb) AS identity, \
    coalesce((SELECT jsonb_agg(m.key ORDER BY m.key) FROM dataset_markings dm \
      JOIN markings m ON m.id = dm.marking_id WHERE dm.dataset_id = d.id), '[]'::jsonb) AS markings";

fn dataset_select() -> String {
    let view = VIEW_TRANSACTIONS
        .replace(" AS_OF_S", "")
        .replace(" AS_OF", "");
    format!(
        "SELECT {DATASET_COLUMNS} FROM datasets d LEFT JOIN LATERAL ( \
           SELECT sum(t.rows) AS rows, sum(t.bytes) AS bytes, max(t.committed_at) AS committed_at \
           FROM dataset_transactions t WHERE {view} \
         ) v ON true"
    )
}

fn dataset_of(row: &QueryResult) -> Result<Dataset, DbErr> {
    let schema: serde_json::Value = row.try_get("", "schema")?;
    let identity: serde_json::Value = row.try_get("", "identity")?;
    Ok(Dataset {
        id: row.try_get::<Uuid>("", "id")?.to_string(),
        name: row.try_get("", "name")?,
        columns: serde_json::from_value(schema).unwrap_or_default(),
        identity: serde_json::from_value(identity).unwrap_or_default(),
        rows: row.try_get("", "rows")?,
        bytes: row.try_get("", "bytes")?,
        committed_at: row.try_get("", "committed_at")?,
        max_age_hours: row.try_get("", "max_age_hours")?,
        stale: row.try_get("", "stale")?,
        import_name: row.try_get("", "import_name")?,
        markings: serde_json::from_value(row.try_get("", "markings")?).unwrap_or_default(),
        created_at: row.try_get("", "created_at")?,
    })
}

// Commit times keep the precision of the column: `as_of` reads a version by its commit
// time, and a rounded time falls before the commit it names.
const TRANSACTION_SELECT: &str = "\
    SELECT id, kind, state, rows, bytes, code, checks, \
    to_char(created_at AT TIME ZONE 'UTC', 'YYYY-MM-DD\"T\"HH24:MI:SS.MS\"Z\"') AS created_at, \
    to_char(committed_at AT TIME ZONE 'UTC', 'YYYY-MM-DD\"T\"HH24:MI:SS.US\"Z\"') AS committed_at \
    FROM dataset_transactions";

fn transaction_of(row: &QueryResult) -> Result<DatasetTransaction, DbErr> {
    let kind: String = row.try_get("", "kind")?;
    let state: String = row.try_get("", "state")?;
    Ok(DatasetTransaction {
        id: row.try_get::<Uuid>("", "id")?.to_string(),
        kind: match kind.as_str() {
            "append" => DatasetTransactionKind::Append,
            _ => DatasetTransactionKind::Snapshot,
        },
        state: match state.as_str() {
            "open" => DatasetTransactionState::Open,
            "committed" => DatasetTransactionState::Committed,
            _ => DatasetTransactionState::Aborted,
        },
        rows: row.try_get("", "rows")?,
        bytes: row.try_get("", "bytes")?,
        code: row.try_get("", "code")?,
        checks: serde_json::from_value(row.try_get("", "checks")?).unwrap_or_default(),
        created_at: row.try_get("", "created_at")?,
        committed_at: row.try_get("", "committed_at")?,
    })
}

pub async fn create(
    db: &DatabaseConnection,
    org_id: i64,
    name: &str,
) -> Result<Dataset, DatasetError> {
    let name = name.trim();
    let invalid = |token: String| DatasetError::Invalid {
        field: "name".into(),
        token,
    };
    if name.is_empty() {
        return Err(invalid("required".into()));
    }
    if name.chars().count() > NAME_MAX {
        return Err(invalid(format!("max_length:{NAME_MAX}")));
    }
    let row = db
        .query_one_raw(stmt(
            "INSERT INTO datasets (org_id, name) VALUES ($1, $2) \
             ON CONFLICT (org_id, name) DO NOTHING RETURNING id",
            vec![org_id.into(), name.into()],
        ))
        .await?
        .ok_or(DatasetError::NameTaken)?;
    get(db, org_id, row.try_get("", "id")?).await
}

pub async fn get(db: &DatabaseConnection, org_id: i64, id: Uuid) -> Result<Dataset, DatasetError> {
    let sql = format!("{} WHERE d.org_id = $1 AND d.id = $2", dataset_select());
    let row = db
        .query_one_raw(stmt(&sql, vec![org_id.into(), id.into()]))
        .await?
        .ok_or(DatasetError::NotFound)?;
    Ok(dataset_of(&row)?)
}

/// How long a dataset may wait for a new version (D576). Setting it starts a new
/// watch: an old warning does not hide the next one.
pub async fn set_freshness(
    db: &DatabaseConnection,
    org_id: i64,
    id: Uuid,
    max_age_hours: Option<i32>,
) -> Result<Dataset, DatasetError> {
    use pnex_core::datasets::FRESHNESS_HOURS_MAX;
    if max_age_hours.is_some_and(|h| !(1..=FRESHNESS_HOURS_MAX).contains(&h)) {
        return Err(DatasetError::Invalid {
            field: "max_age_hours".into(),
            token: "invalid".into(),
        });
    }
    let done = db
        .execute_raw(stmt(
            "UPDATE datasets SET max_age_hours = $3, stale_notified_at = NULL \
             WHERE org_id = $1 AND id = $2",
            vec![org_id.into(), id.into(), max_age_hours.into()],
        ))
        .await?;
    if done.rows_affected() == 0 {
        return Err(DatasetError::NotFound);
    }
    get(db, org_id, id).await
}

/// A page of the org's datasets by name, and their total.
pub async fn list(
    db: &DatabaseConnection,
    org_id: i64,
    limit: i64,
    offset: i64,
) -> Result<(i64, Vec<Dataset>), DatasetError> {
    let total = db
        .query_one_raw(stmt(
            "SELECT count(*) AS n FROM datasets WHERE org_id = $1",
            vec![org_id.into()],
        ))
        .await?
        .map_or(Ok(0), |r| r.try_get("", "n"))?;
    let sql = format!(
        "{} WHERE d.org_id = $1 ORDER BY d.name LIMIT $2 OFFSET $3",
        dataset_select()
    );
    let rows = db
        .query_all_raw(stmt(&sql, vec![org_id.into(), limit.into(), offset.into()]))
        .await?;
    let items = rows.iter().map(dataset_of).collect::<Result<_, _>>()?;
    Ok((total, items))
}

/// A page of a dataset's transactions, newest first, and their total.
pub async fn transactions(
    db: &DatabaseConnection,
    org_id: i64,
    id: Uuid,
    limit: i64,
    offset: i64,
) -> Result<(i64, Vec<DatasetTransaction>), DatasetError> {
    get(db, org_id, id).await?;
    let total = db
        .query_one_raw(stmt(
            "SELECT count(*) AS n FROM dataset_transactions WHERE org_id = $1 AND dataset_id = $2",
            vec![org_id.into(), id.into()],
        ))
        .await?
        .map_or(Ok(0), |r| r.try_get("", "n"))?;
    let sql = format!(
        "{TRANSACTION_SELECT} WHERE org_id = $1 AND dataset_id = $2 \
         ORDER BY created_at DESC, id LIMIT $3 OFFSET $4"
    );
    let rows = db
        .query_all_raw(stmt(
            &sql,
            vec![org_id.into(), id.into(), limit.into(), offset.into()],
        ))
        .await?;
    let items = rows.iter().map(transaction_of).collect::<Result<_, _>>()?;
    Ok((total, items))
}

/// Removes a dataset and its files. Files first: a failure leaves a dataset the
/// caller can delete again, never objects nothing points at.
pub async fn delete(
    db: &DatabaseConnection,
    store: &S3Store,
    org_id: i64,
    id: Uuid,
) -> Result<(), DatasetError> {
    get(db, org_id, id).await?;
    // Recreated at the next run, without what erasures left behind.
    check_loadable(db, org_id, id).await?;
    let pending = db
        .query_one_raw(stmt(
            "SELECT 1 AS x FROM dataset_erasures WHERE dataset_id = $1 AND done_at IS NULL",
            vec![id.into()],
        ))
        .await?;
    if pending.is_some() {
        return Err(DatasetError::ErasurePending);
    }
    let running = db
        .query_one_raw(stmt(
            &format!(
                "SELECT 1 AS x FROM dataset_transactions WHERE dataset_id = $1 AND state = 'open' \
                 AND heartbeat_at > now() - interval '{DEAD_AFTER_MINUTES} minutes'"
            ),
            vec![id.into()],
        ))
        .await?;
    if running.is_some() {
        return Err(DatasetError::TransactionOpen);
    }
    remove_prefix(store, &dataset_prefix(org_id, id)).await?;
    db.execute_raw(stmt(
        "DELETE FROM datasets WHERE org_id = $1 AND id = $2",
        vec![org_id.into(), id.into()],
    ))
    .await?;
    Ok(())
}

async fn remove_prefix(store: &S3Store, prefix: &str) -> Result<usize, DatasetError> {
    let objects = store.list_prefix(prefix).await.map_err(storage)?;
    for (key, _, _) in &objects {
        store.delete_key(key).await.map_err(storage)?;
    }
    Ok(objects.len())
}

/// First rows of the current view, or of the view at `as_of` (RFC 3339).
pub async fn sample(
    db: &DatabaseConnection,
    store: &S3Store,
    org_id: i64,
    id: Uuid,
    limit: usize,
    as_of: Option<chrono::DateTime<chrono::Utc>>,
) -> Result<DatasetSample, DatasetError> {
    let dataset = get(db, org_id, id).await?;
    let mut values: Vec<sea_orm::Value> = vec![id.into()];
    let view = match as_of {
        Some(t) => {
            values.push(t.into());
            VIEW_TRANSACTIONS
                .replace(" AS_OF_S", " AND s.committed_at <= $2")
                .replace(" AS_OF", " AND t.committed_at <= $2")
        }
        None => VIEW_TRANSACTIONS
            .replace(" AS_OF_S", "")
            .replace(" AS_OF", ""),
    };
    let files = db
        .query_all_raw(stmt(
            &format!(
                "SELECT f.key, f.bytes, t.purged_at IS NOT NULL AS purged \
                 FROM datasets d \
                 JOIN dataset_transactions t ON {view} \
                 JOIN dataset_files f ON f.transaction_id = t.id \
                 WHERE d.id = $1 ORDER BY t.seq, f.key"
            ),
            values,
        ))
        .await?;
    let mut names = Vec::new();
    let mut rows = Vec::new();
    let limit = limit.clamp(1, SAMPLE_ROWS_MAX);
    for file in &files {
        if file.try_get::<bool>("", "purged")? {
            return Err(DatasetError::VersionPurged);
        }
        if rows.len() >= limit {
            continue;
        }
        let key: String = file.try_get("", "key")?;
        let size: i64 = file.try_get("", "bytes")?;
        let (file_names, file_rows) = parquet::sample(store, &key, size as u64, limit - rows.len())
            .await
            .map_err(storage)?;
        names = file_names;
        rows.extend(file_rows);
    }
    if names.is_empty() {
        names = dataset.columns.into_iter().map(|c| c.name).collect();
    }
    Ok(DatasetSample {
        columns: names,
        rows,
    })
}

/// How full each column of the current view is, from the footers of its files.
pub async fn profile(
    db: &DatabaseConnection,
    store: &S3Store,
    org_id: i64,
    id: Uuid,
) -> Result<pnex_core::datasets::DatasetProfile, DatasetError> {
    use pnex_core::datasets::{DatasetColumnProfile, DatasetProfile};
    let dataset = get(db, org_id, id).await?;
    let mut columns: Vec<DatasetColumnProfile> = dataset
        .columns
        .iter()
        .map(|c| DatasetColumnProfile {
            name: c.name.clone(),
            data_type: c.data_type.clone(),
            nulls: Some(0),
        })
        .collect();
    for file in current_files(db, org_id, id).await? {
        let counts = parquet::null_counts(store, &file.key, file.bytes)
            .await
            .map_err(storage)?;
        for column in &mut columns {
            let found = counts.iter().find(|(name, _)| *name == column.name);
            // A file without this column has nothing but empty values, which its row
            // count alone could tell: unknown is the honest answer.
            let nulls = found.and_then(|(_, n)| *n);
            column.nulls = column.nulls.zip(nulls).map(|(a, b)| a + b);
        }
    }
    Ok(DatasetProfile {
        rows: dataset.rows,
        columns,
    })
}

/// A file of a view.
pub struct ViewFile {
    pub transaction: Uuid,
    pub key: String,
    pub bytes: u64,
    pub rows: u64,
}

/// Files of the current view of a dataset of the org, in commit order.
pub async fn current_files(
    db: &DatabaseConnection,
    org_id: i64,
    id: Uuid,
) -> Result<Vec<ViewFile>, DatasetError> {
    view_files(db, org_id, id, None).await
}

/// Files of the view of a dataset of the org, current or at `as_of`, in
/// commit order.
pub async fn view_files(
    db: &DatabaseConnection,
    org_id: i64,
    id: Uuid,
    as_of: Option<chrono::DateTime<chrono::Utc>>,
) -> Result<Vec<ViewFile>, DatasetError> {
    get(db, org_id, id).await?;
    let mut values: Vec<sea_orm::Value> = vec![id.into()];
    let view = match as_of {
        Some(t) => {
            values.push(t.into());
            VIEW_TRANSACTIONS
                .replace(" AS_OF_S", " AND s.committed_at <= $2")
                .replace(" AS_OF", " AND t.committed_at <= $2")
        }
        None => VIEW_TRANSACTIONS
            .replace(" AS_OF_S", "")
            .replace(" AS_OF", ""),
    };
    let rows = db
        .query_all_raw(stmt(
            &format!(
                "SELECT t.id, f.key, f.bytes, f.rows, t.purged_at IS NOT NULL AS purged \
                 FROM datasets d \
                 JOIN dataset_transactions t ON {view} \
                 JOIN dataset_files f ON f.transaction_id = t.id \
                 WHERE d.id = $1 ORDER BY t.seq, f.key"
            ),
            values,
        ))
        .await?;
    rows.iter().map(view_file).collect()
}

/// Files of these transactions of a dataset of the org, in commit order, or
/// `None` when one has left storage: that version can no longer be read.
pub async fn files_of_transactions(
    db: &DatabaseConnection,
    org_id: i64,
    id: Uuid,
    transactions: &[Uuid],
) -> Result<Option<Vec<ViewFile>>, DatasetError> {
    let rows = db
        .query_all_raw(stmt(
            "SELECT t.id, f.key, f.bytes, f.rows, t.purged_at IS NOT NULL AS purged \
             FROM dataset_transactions t JOIN dataset_files f ON f.transaction_id = t.id \
             WHERE t.org_id = $1 AND t.dataset_id = $2 AND t.state = 'committed' AND t.id = ANY($3) \
             ORDER BY t.seq, f.key",
            vec![org_id.into(), id.into(), transactions.to_vec().into()],
        ))
        .await?;
    let files: Result<Vec<ViewFile>, DatasetError> = rows.iter().map(view_file).collect();
    match files {
        // A transaction of another dataset, or one with no file: no version to read.
        Ok(files)
            if files
                .iter()
                .map(|f| f.transaction)
                .collect::<std::collections::HashSet<_>>()
                .len()
                != transactions.len() =>
        {
            Ok(None)
        }
        Ok(files) => Ok(Some(files)),
        Err(DatasetError::VersionPurged) => Ok(None),
        Err(e) => Err(e),
    }
}

fn view_file(row: &QueryResult) -> Result<ViewFile, DatasetError> {
    if row.try_get::<bool>("", "purged")? {
        return Err(DatasetError::VersionPurged);
    }
    Ok(ViewFile {
        transaction: row.try_get("", "id")?,
        key: row.try_get("", "key")?,
        bytes: row.try_get::<i64>("", "bytes")?.max(0) as u64,
        rows: row.try_get::<i64>("", "rows")?.max(0) as u64,
    })
}

/// Bytes of the org's committed files still in storage.
pub async fn used_bytes(db: &DatabaseConnection, org_id: i64) -> Result<u64, DbErr> {
    let row = db
        .query_one_raw(stmt(
            "SELECT coalesce(sum(bytes), 0)::bigint AS b FROM dataset_transactions \
             WHERE org_id = $1 AND state = 'committed' AND purged_at IS NULL",
            vec![org_id.into()],
        ))
        .await?;
    Ok(row.map_or(Ok(0), |r| r.try_get::<i64>("", "b"))?.max(0) as u64)
}

fn quota_key(org_id: i64) -> String {
    format!("dataset_quota_bytes:{org_id}")
}

/// The quota a platform admin set for the org, `None` = [`QUOTA_BYTES`].
pub async fn quota_override(db: &DatabaseConnection, org_id: i64) -> Result<Option<u64>, DbErr> {
    let row = db
        .query_one_raw(stmt(
            "SELECT value FROM system_settings WHERE key = $1",
            vec![quota_key(org_id).into()],
        ))
        .await?;
    Ok(row
        .and_then(|r| r.try_get::<String>("", "value").ok())
        .and_then(|v| v.parse().ok()))
}

/// Sets (or with `None` drops) the org's quota. Platform admin only (D821):
/// every member owns a personal org, an org admin never raises its own.
pub async fn set_quota(
    db: &DatabaseConnection,
    org_id: i64,
    bytes: Option<u64>,
    by: i64,
) -> Result<(), DbErr> {
    let key = quota_key(org_id);
    match bytes {
        Some(b) => db.execute_raw(stmt(
            "INSERT INTO system_settings (key, value, updated_by, created_at, updated_at) \
             VALUES ($1, $2, $3, now(), now()) ON CONFLICT (key) DO UPDATE SET \
               value = EXCLUDED.value, updated_by = EXCLUDED.updated_by, updated_at = now()",
            vec![key.into(), b.to_string().into(), by.into()],
        )),
        None => db.execute_raw(stmt(
            "DELETE FROM system_settings WHERE key = $1",
            vec![key.into()],
        )),
    }
    .await?;
    Ok(())
}

/// Bytes the org may still write, refused when none.
async fn room(db: &DatabaseConnection, org_id: i64) -> Result<u64, DatasetError> {
    let quota = quota_override(db, org_id).await?.unwrap_or(QUOTA_BYTES);
    quota
        .checked_sub(used_bytes(db, org_id).await?)
        .filter(|n| *n > 0)
        .ok_or(DatasetError::QuotaExceeded)
}

/// Opens a transaction, after retiring a dead one of this dataset.
async fn begin(
    db: &DatabaseConnection,
    org_id: i64,
    id: Uuid,
    kind: DatasetTransactionKind,
) -> Result<Uuid, DatasetError> {
    db.execute_raw(stmt(
        &format!(
            "UPDATE dataset_transactions SET state = 'aborted', code = $2 \
             WHERE dataset_id = $1 AND state = 'open' \
             AND heartbeat_at < now() - interval '{DEAD_AFTER_MINUTES} minutes'"
        ),
        vec![id.into(), err_codes::DATASET_TRANSACTION_ABORTED.into()],
    ))
    .await?;
    let row = db
        .query_one_raw(stmt(
            "INSERT INTO dataset_transactions (org_id, dataset_id, kind) VALUES ($1, $2, $3) \
             ON CONFLICT (dataset_id) WHERE state = 'open' DO NOTHING RETURNING id",
            vec![org_id.into(), id.into(), kind.as_str().into()],
        ))
        .await?
        .ok_or(DatasetError::TransactionOpen)?;
    Ok(row.try_get("", "id")?)
}

/// What a load wrote.
struct Written {
    key: String,
    bytes: u64,
    rows: u64,
    sha256: String,
    columns: Vec<DatasetColumn>,
}

/// Streams a CSV body into one Parquet object.
#[allow(clippy::too_many_arguments)]
async fn write_csv<S, E>(
    db: &DatabaseConnection,
    store: &S3Store,
    key: &str,
    transaction_id: Uuid,
    expected: Option<&[DatasetColumn]>,
    room: u64,
    delimiter: Option<u8>,
    erased: &parquet::Targets,
    mut body: S,
) -> Result<Written, DatasetError>
where
    S: Stream<Item = Result<Bytes, E>> + Unpin,
{
    let mut csv = CsvBatches::new(delimiter);
    let mut writer: Option<AsyncArrowWriter<S3Sink>> = None;
    let out = async {
        let (mut read, mut rows) = (0u64, 0u64);
        let mut beat = Instant::now();
        let mut ended = false;
        while !ended {
            let batches = match body.next().await {
                Some(Ok(chunk)) => {
                    read += chunk.len() as u64;
                    if read > SOURCE_MAX_BYTES {
                        return Err(DatasetError::SourceTooLarge);
                    }
                    csv.push(&chunk).map_err(DatasetError::Csv)?
                }
                Some(Err(_)) => return Err(DatasetError::SourceInterrupted),
                None => {
                    ended = true;
                    csv.finish().map_err(DatasetError::Csv)?
                }
            };
            if writer.is_none() {
                if let Some(schema) = csv.schema() {
                    if let Some(want) = expected {
                        let got: Vec<&String> = schema.fields().iter().map(|f| f.name()).collect();
                        let same = got.len() == want.len()
                            && got.iter().zip(want).all(|(g, w)| **g == w.name);
                        if !same {
                            return Err(DatasetError::SchemaMismatch {
                                columns: want.iter().map(|c| c.name.clone()).collect(),
                            });
                        }
                    }
                    let properties = WriterProperties::builder()
                        .set_compression(Compression::ZSTD(ZstdLevel::default()))
                        .build();
                    let sink = S3Sink::new(store.writer(key).await.map_err(storage)?);
                    writer = Some(
                        AsyncArrowWriter::try_new(sink, schema, Some(properties))
                            .map_err(storage)?,
                    );
                }
            }
            if let Some(w) = writer.as_mut() {
                for batch in &batches {
                    let kept;
                    let batch = if erased.is_empty() {
                        batch
                    } else {
                        kept = parquet::without(batch, erased).map_err(storage)?;
                        &kept
                    };
                    rows += batch.num_rows() as u64;
                    w.write(batch).await.map_err(storage)?;
                    // The encoded estimate misses unencoded buffers and dictionaries.
                    if w.in_progress_size().max(w.memory_size()) > ROW_GROUP_BYTES {
                        w.flush().await.map_err(storage)?;
                    }
                    if w.bytes_written() as u64 > room {
                        return Err(DatasetError::QuotaExceeded);
                    }
                }
            }
            if beat.elapsed() > HEARTBEAT_EVERY {
                beat = Instant::now();
                db.execute_raw(stmt(
                    "UPDATE dataset_transactions SET heartbeat_at = now() WHERE id = $1",
                    vec![transaction_id.into()],
                ))
                .await?;
            }
        }
        Ok(rows)
    }
    .await;
    let rows = match out {
        Ok(rows) => rows,
        Err(e) => {
            if let Some(w) = writer {
                w.into_inner().abort().await;
            }
            return Err(e);
        }
    };
    let mut w = writer.ok_or(DatasetError::Csv(CsvError::Empty))?;
    let columns = csv
        .schema()
        .map(|s| {
            s.fields()
                .iter()
                .map(|f| DatasetColumn {
                    name: f.name().clone(),
                    data_type: "text".into(),
                })
                .collect()
        })
        .unwrap_or_default();
    if let Err(e) = w.finish().await {
        w.into_inner().abort().await;
        return Err(storage(e));
    }
    let sink = w.into_inner();
    let bytes = sink.bytes;
    Ok(Written {
        key: key.to_string(),
        bytes,
        rows,
        sha256: sink.sha256(),
        columns,
    })
}

/// Records the file and makes the transaction visible, as one PostgreSQL write.
/// A snapshot gives the dataset its columns; so does a first append.
async fn commit(
    db: &DatabaseConnection,
    dataset_id: Uuid,
    transaction_id: Uuid,
    kind: DatasetTransactionKind,
    written: &Written,
) -> Result<(), DatasetError> {
    let tx = db.begin().await?;
    tx.execute_raw(stmt(
        "SELECT id FROM datasets WHERE id = $1 FOR UPDATE",
        vec![dataset_id.into()],
    ))
    .await?;
    let done = tx
        .execute_raw(stmt(
            "UPDATE dataset_transactions SET state = 'committed', \
             seq = nextval('dataset_commit_seq'), committed_at = now(), rows = $2, bytes = $3 \
             WHERE id = $1 AND state = 'open'",
            vec![
                transaction_id.into(),
                (written.rows as i64).into(),
                (written.bytes as i64).into(),
            ],
        ))
        .await?;
    if done.rows_affected() == 0 {
        return Err(DatasetError::TransactionAborted);
    }
    tx.execute_raw(stmt(
        "INSERT INTO dataset_files (transaction_id, key, bytes, rows, sha256) \
         VALUES ($1, $2, $3, $4, $5)",
        vec![
            transaction_id.into(),
            written.key.clone().into(),
            (written.bytes as i64).into(),
            (written.rows as i64).into(),
            written.sha256.clone().into(),
        ],
    ))
    .await?;
    let columns = serde_json::to_value(&written.columns).unwrap_or_default();
    let snapshot = kind == DatasetTransactionKind::Snapshot;
    tx.execute_raw(stmt(
        "UPDATE datasets SET schema = $2 WHERE id = $1 AND ($3 OR schema = '[]'::jsonb)",
        vec![dataset_id.into(), columns.into(), snapshot.into()],
    ))
    .await?;
    tx.commit().await?;
    Ok(())
}

/// Refuses what a person or a run may not load: the table an import's steps fill
/// feeds a sync written with the role of the import's author.
pub async fn check_loadable(
    db: &DatabaseConnection,
    org_id: i64,
    id: Uuid,
) -> Result<(), DatasetError> {
    let owned = db
        .query_one_raw(stmt(
            "SELECT import_id IS NOT NULL AS owned FROM datasets WHERE org_id = $1 AND id = $2",
            vec![org_id.into(), id.into()],
        ))
        .await?
        .ok_or(DatasetError::NotFound)?;
    match owned.try_get("", "owned")? {
        true => Err(DatasetError::Reserved),
        false => Ok(()),
    }
}

/// Loads a CSV body as a new transaction: written in a stream, in memory bounded
/// by the row group, and visible only once committed.
pub async fn load_csv<S, E>(
    db: &DatabaseConnection,
    store: &S3Store,
    org_id: i64,
    id: Uuid,
    kind: DatasetTransactionKind,
    delimiter: Option<u8>,
    body: S,
) -> Result<DatasetTransaction, DatasetError>
where
    S: Stream<Item = Result<Bytes, E>> + Unpin,
{
    let dataset = get(db, org_id, id).await?;
    let room = room(db, org_id).await?;
    // In this order: an erasure queued after `begin` waits for this transaction,
    // one queued before is read here.
    let transaction_id = begin(db, org_id, id, kind).await?;
    let erased = erasure::targets_of(db, id).await?;
    let key = format!(
        "{}part-0.parquet",
        transaction_prefix(org_id, id, transaction_id)
    );
    let expected = (kind == DatasetTransactionKind::Append && !dataset.columns.is_empty())
        .then_some(dataset.columns.as_slice());
    let result: Result<Option<Uuid>, DatasetError> = async {
        let written = write_csv(
            db,
            store,
            &key,
            transaction_id,
            expected,
            room,
            delimiter,
            &erased,
            body,
        )
        .await?;
        if kind == DatasetTransactionKind::Snapshot {
            if let Some(same) = current_snapshot_with(db, id, &written.sha256).await? {
                return Ok(Some(same));
            }
        }
        commit(db, id, transaction_id, kind, &written).await?;
        Ok(None)
    }
    .await;
    // The same file night after night is not a new version: without this, a
    // daily source fills the quota with copies during the retention.
    if let Ok(Some(same)) = result {
        db.execute_raw(stmt(
            "UPDATE dataset_transactions SET state = 'aborted', code = $2 \
             WHERE id = $1 AND state = 'open'",
            vec![transaction_id.into(), err_codes::DATASET_UNCHANGED.into()],
        ))
        .await?;
        return transaction_by_id(db, same).await;
    }
    if let Err(e) = result {
        // The purge removes what the transaction wrote.
        db.execute_raw(stmt(
            "UPDATE dataset_transactions SET state = 'aborted', code = $2 \
             WHERE id = $1 AND state = 'open'",
            vec![transaction_id.into(), e.code().into()],
        ))
        .await?;
        return Err(e);
    }
    transaction_by_id(db, transaction_id).await
}

/// Appends rows our own code typed (a decoded price file, D809) as one
/// transaction: one Parquet file, visible only once committed. The first
/// append gives the dataset its columns; a later one must have the same.
pub async fn append_batch(
    db: &DatabaseConnection,
    store: &S3Store,
    org_id: i64,
    id: Uuid,
    batch: &arrow_array::RecordBatch,
) -> Result<DatasetTransaction, DatasetError> {
    use arrow_schema::DataType;
    let dataset = get(db, org_id, id).await?;
    let columns: Vec<DatasetColumn> = batch
        .schema()
        .fields()
        .iter()
        .map(|f| DatasetColumn {
            name: f.name().clone(),
            // Same names as the pipeline worker's.
            data_type: match f.data_type() {
                DataType::Utf8 => "text".into(),
                DataType::Timestamp(..) => "timestamp".into(),
                DataType::Decimal128(..) => "decimal".into(),
                other => format!("{other:?}").to_lowercase(),
            },
        })
        .collect();
    if !dataset.columns.is_empty() && dataset.columns != columns {
        return Err(DatasetError::SchemaMismatch {
            columns: dataset.columns.iter().map(|c| c.name.clone()).collect(),
        });
    }
    let room = room(db, org_id).await?;
    let kind = DatasetTransactionKind::Append;
    let transaction_id = begin(db, org_id, id, kind).await?;
    let key = format!(
        "{}part-0.parquet",
        transaction_prefix(org_id, id, transaction_id)
    );
    let result = async {
        let erased = erasure::targets_of(db, id).await?;
        let kept;
        let batch = if erased.is_empty() {
            batch
        } else {
            kept = parquet::without(batch, &erased).map_err(storage)?;
            &kept
        };
        let properties = WriterProperties::builder()
            .set_compression(Compression::ZSTD(ZstdLevel::default()))
            .build();
        let sink = S3Sink::new(store.writer(&key).await.map_err(storage)?);
        let mut w =
            AsyncArrowWriter::try_new(sink, batch.schema(), Some(properties)).map_err(storage)?;
        let written = async {
            w.write(batch).await.map_err(storage)?;
            w.flush().await.map_err(storage)?;
            if w.bytes_written() as u64 > room {
                return Err(DatasetError::QuotaExceeded);
            }
            Ok(())
        }
        .await;
        if let Err(e) = written.and(w.finish().await.map(|_| ()).map_err(storage)) {
            w.into_inner().abort().await;
            return Err(e);
        }
        let sink = w.into_inner();
        let written = Written {
            key: key.clone(),
            bytes: sink.bytes,
            rows: batch.num_rows() as u64,
            sha256: sink.sha256(),
            columns,
        };
        commit(db, id, transaction_id, kind, &written).await
    }
    .await;
    if let Err(e) = result {
        // The purge removes what the transaction wrote.
        db.execute_raw(stmt(
            "UPDATE dataset_transactions SET state = 'aborted', code = $2 \
             WHERE id = $1 AND state = 'open'",
            vec![transaction_id.into(), e.code().into()],
        ))
        .await?;
        return Err(e);
    }
    transaction_by_id(db, transaction_id).await
}

async fn transaction_by_id(
    db: &DatabaseConnection,
    id: Uuid,
) -> Result<DatasetTransaction, DatasetError> {
    let row = db
        .query_one_raw(stmt(
            &format!("{TRANSACTION_SELECT} WHERE id = $1"),
            vec![id.into()],
        ))
        .await?
        .ok_or(DatasetError::NotFound)?;
    Ok(transaction_of(&row)?)
}

/// The latest transaction when it is a snapshot made of this one file.
async fn current_snapshot_with(
    db: &DatabaseConnection,
    id: Uuid,
    sha256: &str,
) -> Result<Option<Uuid>, DbErr> {
    let row = db
        .query_one_raw(stmt(
            "SELECT t.id FROM ( \
               SELECT id, kind FROM dataset_transactions \
               WHERE dataset_id = $1 AND state = 'committed' AND purged_at IS NULL \
               ORDER BY seq DESC LIMIT 1) t \
             WHERE t.kind = 'snapshot' \
             AND (SELECT count(*) FROM dataset_files f WHERE f.transaction_id = t.id) = 1 \
             AND EXISTS (SELECT 1 FROM dataset_files f \
                         WHERE f.transaction_id = t.id AND f.sha256 = $2)",
            vec![id.into(), sha256.into()],
        ))
        .await?;
    row.map(|r| r.try_get("", "id")).transpose()
}

/// One pass: retires dead transactions, removes the files of aborted ones, and
/// of replaced versions past their retention. Returns the objects removed.
pub async fn purge(db: &DatabaseConnection, store: &S3Store) -> Result<usize, DatasetError> {
    db.execute_raw(stmt(
        &format!(
            "UPDATE dataset_transactions SET state = 'aborted', code = $1 \
             WHERE state = 'open' AND heartbeat_at < now() - interval '{DEAD_AFTER_MINUTES} minutes'"
        ),
        vec![err_codes::DATASET_TRANSACTION_ABORTED.into()],
    ))
    .await?;
    let due = db
        .query_all_raw(stmt(
            &format!(
                "SELECT t.id, t.org_id, t.dataset_id FROM dataset_transactions t \
                 JOIN datasets d ON d.id = t.dataset_id \
                 WHERE t.purged_at IS NULL AND ( \
                   t.state = 'aborted' \
                   OR (t.state = 'committed' \
                       AND t.committed_at < now() \
                         - interval '1 day' * coalesce(d.retain_days, {RETAIN_DAYS}) \
                       AND t.seq < (SELECT max(s.seq) FROM dataset_transactions s \
                                    WHERE s.dataset_id = t.dataset_id AND s.state = 'committed' \
                                    AND s.kind = 'snapshot')) \
                 ) LIMIT $1"
            ),
            vec![PURGE_BATCH.into()],
        ))
        .await?;
    let mut removed = uploads::sweep(db, store).await?;
    for row in &due {
        let id: Uuid = row.try_get("", "id")?;
        let prefix = transaction_prefix(
            row.try_get("", "org_id")?,
            row.try_get("", "dataset_id")?,
            id,
        );
        removed += remove_prefix(store, &prefix).await?;
        db.execute_raw(stmt(
            "UPDATE dataset_transactions SET purged_at = now() WHERE id = $1",
            vec![id.into()],
        ))
        .await?;
    }
    Ok(removed)
}

pub fn spawn_purge(ctx: &loco_rs::app::AppContext) {
    let db = ctx.db.clone();
    let config = ctx.config.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_secs(300)).await;
        loop {
            if crate::services::singleton::my_turn(&db, "dataset-purge", PURGE_EVERY).await {
                match store_of(&config) {
                    Ok(store) => match purge(&db, &store).await {
                        Ok(0) => {}
                        Ok(n) => tracing::info!(deleted = n, "dataset purge"),
                        Err(e) => tracing::warn!("dataset purge failed: {e:?}"),
                    },
                    Err(e) => tracing::debug!("dataset purge skipped: {e:?}"),
                }
                match S3Config::from_config(&config)
                    .abort_stale_uploads("org_", UPLOAD_ABANDONED_AFTER)
                    .await
                {
                    Ok(0) => {}
                    Ok(n) => tracing::info!(aborted = n, "uploads left behind"),
                    Err(e) => tracing::warn!("uploads not swept: {e}"),
                }
            }
            tokio::time::sleep(PURGE_EVERY).await;
        }
    });
}
