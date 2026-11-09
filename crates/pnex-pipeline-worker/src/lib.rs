//! Runs of data pipelines (data-pipelines.md D571–D574): a queue in PostgreSQL,
//! SQL transforms over datasets, each output committed as a transaction with the
//! transactions it read. This process holds no secret but its catalogue and S3
//! access, and sees no business table. Keys and commits follow
//! `pnex-backend/src/services/datasets`, which purges what this writes.

mod analysis;
pub mod engine;

use std::collections::{BTreeSet, HashMap, HashSet};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use bytes::Bytes;
use datafusion::arrow::array::{cast::AsArray, BooleanArray, RecordBatch};
use datafusion::arrow::compute::{can_cast_types, cast, filter_record_batch};
use datafusion::arrow::datatypes::{DataType, Int64Type, SchemaRef};
use datafusion::arrow::error::ArrowError;
use datafusion::dataframe::DataFrame;
use datafusion::error::DataFusionError;
use datafusion::parquet::arrow::async_writer::{AsyncArrowWriter, AsyncFileWriter};
use datafusion::parquet::basic::{Compression, ZstdLevel};
use datafusion::parquet::errors::{ParquetError, Result as ParquetResult};
use datafusion::parquet::file::properties::WriterProperties;
use datafusion::prelude::{ParquetReadOptions, SessionContext};
use futures::future::BoxFuture;
use futures::{FutureExt, StreamExt};
use object_store::aws::{AmazonS3, AmazonS3Builder};
use object_store::buffered::BufWriter;
use object_store::path::Path as StorePath;
use pnex_core::err_codes;
use pnex_core::pipelines::{CheckResult, PipelineDocument, Transform};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use sqlx::postgres::{PgConnectOptions, PgPoolOptions};
use sqlx::{PgPool, Row};
use tokio::io::AsyncWriteExt;
use uuid::Uuid;

use self::engine::Limits;

const HEARTBEAT_EVERY: Duration = Duration::from_secs(30);
/// A run that took its worker down twice gets no third try: each one would
/// take the worker down again (dev, 2026-11-06).
const MAX_ATTEMPTS: i32 = 2;
const ROW_GROUP_BYTES: usize = 64 * 1024 * 1024;
/// Same quota as `services/datasets`.
const QUOTA_BYTES: u64 = 20 * 1024 * 1024 * 1024;

#[derive(Clone, Debug)]
pub struct Config {
    /// Connects as `pnex_pipeline_worker` in production (D573).
    pub database_url: String,
    /// `SET ROLE` on every connection, for a login that holds the role.
    pub database_role: Option<String>,
    pub s3_endpoint: String,
    pub s3_bucket: String,
    pub s3_region: String,
    pub s3_access_key: String,
    pub s3_secret_key: String,
    pub s3_path_style: bool,
    pub limits: Limits,
    pub transform_timeout: Duration,
    /// All the transforms of a run together: the worker runs one at a time, and a
    /// long document must not hold every other org back.
    pub run_timeout: Duration,
    /// Master key of `pseudonym()` (governance.md D805); without it the function
    /// does not exist and a transform calling it fails.
    pub pseudonym_key: Option<Vec<u8>>,
}

fn env_or<T: std::str::FromStr>(name: &str, default: T) -> T {
    std::env::var(name)
        .ok()
        // Compose passes an unset variable as an empty one.
        .filter(|v| !v.is_empty())
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}

impl Config {
    pub fn from_env() -> Result<Self, String> {
        let need = |name: &str| std::env::var(name).map_err(|_| format!("{name} is required"));
        Ok(Self {
            database_url: need("PNEX_PIPELINE_DATABASE_URL")?,
            database_role: std::env::var("PNEX_PIPELINE_DATABASE_ROLE").ok(),
            s3_endpoint: need("PNEX_S3_ENDPOINT")?,
            s3_bucket: need("PNEX_S3_BUCKET")?,
            s3_region: env_or("PNEX_S3_REGION", "us-east-1".to_string()),
            s3_access_key: need("PNEX_S3_ACCESS_KEY")?,
            s3_secret_key: need("PNEX_S3_SECRET_KEY")?,
            s3_path_style: env_or("PNEX_S3_PATH_STYLE", true),
            limits: Limits {
                // §9.7 bis: a pod of 4 GB, the pool at 75 %.
                memory_bytes: env_or("PNEX_PIPELINE_MEMORY_MB", 3072usize) * 1024 * 1024,
                partitions: env_or("PNEX_PIPELINE_PARTITIONS", 4usize),
                spill_dir: env_or(
                    "PNEX_PIPELINE_SPILL_DIR",
                    std::env::temp_dir().join("pnex-pipeline-spill"),
                ),
                spill_max_bytes: env_or("PNEX_PIPELINE_SPILL_MAX_GB", 50u64) * 1024 * 1024 * 1024,
            },
            transform_timeout: Duration::from_secs(env_or(
                "PNEX_PIPELINE_TRANSFORM_TIMEOUT_SECS",
                3600u64,
            )),
            run_timeout: Duration::from_secs(env_or("PNEX_PIPELINE_RUN_TIMEOUT_SECS", 4 * 3600u64)),
            // At least 32 bytes: a short key would make the stand-ins guessable.
            pseudonym_key: std::env::var("PNEX_PSEUDONYM_KEY")
                .ok()
                .filter(|k| k.len() >= 32)
                .map(String::into_bytes),
        })
    }
}

/// Why a run stopped: a machine code and its arguments.
#[derive(Debug)]
pub struct Failure {
    pub code: &'static str,
    pub args: Value,
    /// The hash table of a join outgrew the pool: a sort-merge join may pass.
    pub hash_join: bool,
}

impl Failure {
    fn new(code: &'static str, args: Value) -> Self {
        Self {
            code,
            args,
            hash_join: false,
        }
    }

    /// Infrastructure failed, not the pipeline. The detail stays in the logs: it
    /// can name hosts and buckets.
    fn storage(e: impl std::fmt::Display) -> Self {
        tracing::warn!("pipeline storage: {e}");
        Self::new(err_codes::DATASET_STORAGE_UNAVAILABLE, Value::Null)
    }

    /// An error of the engine while it runs a user query. Only an error about the
    /// query itself is shown: one met while reading can name a bucket and a key, or
    /// quote a cell, and the run keeps what it is given.
    fn of_engine(output: &str, e: DataFusionError) -> Self {
        let root = e.find_root();
        match root {
            DataFusionError::ResourcesExhausted(_) => Self {
                hash_join: engine::hash_join_too_big(&e),
                ..Self::new(
                    err_codes::PIPELINE_MEMORY_EXCEEDED,
                    json!({ "output": output }),
                )
            },
            DataFusionError::ObjectStore(_)
            | DataFusionError::IoError(_)
            | DataFusionError::ParquetError(_)
            | DataFusionError::External(_) => Self::storage(&e),
            DataFusionError::Plan(_)
            | DataFusionError::SQL(..)
            | DataFusionError::SchemaError(..)
            | DataFusionError::NotImplemented(_)
            | DataFusionError::Configuration(_) => Self::new(
                err_codes::PIPELINE_SQL_INVALID,
                json!({ "output": output, "detail": cut(&root.to_string()) }),
            ),
            _ => {
                tracing::info!("pipeline query stopped: {e}");
                Self::new(err_codes::PIPELINE_SQL_INVALID, json!({ "output": output }))
            }
        }
    }
}

/// The engine message goes to the user as a verbatim diagnostic, bounded.
fn cut(s: &str) -> String {
    s.chars().take(1000).collect()
}

fn db_failure(e: sqlx::Error) -> Failure {
    Failure::storage(e)
}

/// Streams a Parquet file to S3 while hashing it.
struct Sink {
    writer: BufWriter,
    hash: Sha256,
    bytes: u64,
}

impl AsyncFileWriter for Sink {
    fn write(&mut self, bs: Bytes) -> BoxFuture<'_, ParquetResult<()>> {
        self.hash.update(&bs);
        self.bytes += bs.len() as u64;
        async move {
            self.writer
                .write_all(&bs)
                .await
                .map_err(|e| ParquetError::External(Box::new(e)))
        }
        .boxed()
    }

    fn complete(&mut self) -> BoxFuture<'_, ParquetResult<()>> {
        async move {
            self.writer
                .shutdown()
                .await
                .map_err(|e| ParquetError::External(Box::new(e)))
        }
        .boxed()
    }
}

/// One input as the transform reads it.
struct Input {
    alias: String,
    /// Transactions of its view: what the output is computed from.
    transactions: BTreeSet<Uuid>,
    files: Vec<String>,
}

pub struct Worker {
    pool: PgPool,
    store: Arc<AmazonS3>,
    config: Config,
    /// The job and the output transaction in progress, for the heartbeat.
    current: Arc<Mutex<(Option<Uuid>, Option<Uuid>)>>,
}

impl Worker {
    pub async fn connect(config: Config) -> Result<Self, String> {
        let options: PgConnectOptions = config
            .database_url
            .parse()
            .map_err(|e: sqlx::Error| e.to_string())?;
        let options = match &config.database_role {
            Some(role) => options.options([("role", role.as_str())]),
            None => options,
        };
        let pool = PgPoolOptions::new()
            .max_connections(4)
            .connect_with(options)
            .await
            .map_err(|e| e.to_string())?;
        let store = AmazonS3Builder::new()
            .with_bucket_name(&config.s3_bucket)
            .with_region(&config.s3_region)
            .with_endpoint(&config.s3_endpoint)
            .with_access_key_id(&config.s3_access_key)
            .with_secret_access_key(&config.s3_secret_key)
            .with_allow_http(config.s3_endpoint.starts_with("http://"))
            .with_virtual_hosted_style_request(!config.s3_path_style)
            .build()
            .map_err(|e| e.to_string())?;
        std::fs::create_dir_all(&config.limits.spill_dir).map_err(|e| e.to_string())?;
        let current = Arc::new(Mutex::new((None, None)));
        let worker = Self {
            pool,
            store: Arc::new(store),
            config,
            current,
        };
        worker.spawn_heartbeat();
        Ok(worker)
    }

    fn spawn_heartbeat(&self) {
        let (pool, current) = (self.pool.clone(), self.current.clone());
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(HEARTBEAT_EVERY).await;
                let (job, transaction) = *current.lock().expect("heartbeat lock");
                let Some(job) = job else { continue };
                let beat =
                    sqlx::query("UPDATE pipeline_jobs SET heartbeat_at = now() WHERE id = $1")
                        .bind(job)
                        .execute(&pool)
                        .await;
                if let Err(e) = beat {
                    tracing::warn!("pipeline heartbeat: {e}");
                }
                if let Some(t) = transaction {
                    let _ = sqlx::query(
                        "UPDATE dataset_transactions SET heartbeat_at = now() WHERE id = $1",
                    )
                    .bind(t)
                    .execute(&pool)
                    .await;
                }
            }
        });
    }

    /// Gives the run in progress back to the queue (SIGTERM): the next worker
    /// skips the outputs already committed.
    pub async fn release(&self) {
        let (job, transaction) = *self.current.lock().expect("heartbeat lock");
        if let Some(t) = transaction {
            let _ = sqlx::query(
                "UPDATE dataset_transactions SET state = 'aborted', code = $2 \
                 WHERE id = $1 AND state = 'open'",
            )
            .bind(t)
            .bind(err_codes::DATASET_TRANSACTION_ABORTED)
            .execute(&self.pool)
            .await;
        }
        if let Some(j) = job {
            let _ = sqlx::query(
                "UPDATE pipeline_jobs SET state = 'queued', attempts = attempts - 1 \
                 WHERE id = $1 AND state = 'running'",
            )
            .bind(j)
            .execute(&self.pool)
            .await;
        }
    }

    /// Runs dead for too long go back to the queue, or fail after their attempts.
    async fn sweep_dead(&self) -> Result<(), sqlx::Error> {
        // One minute past the 10 after which `begin()` frees the open transaction of a
        // dead run (its last beat may follow the job's by 30 s): requeued sooner, the
        // retry would find it still open (dataset-transaction-open).
        sqlx::query(
            "UPDATE pipeline_jobs SET state = 'failed', code = $1, finished_at = now() \
             WHERE state = 'running' AND heartbeat_at < now() - interval '11 minutes' \
             AND attempts >= $2",
        )
        .bind(err_codes::PIPELINE_WORKER_LOST)
        .bind(MAX_ATTEMPTS)
        .execute(&self.pool)
        .await?;
        sqlx::query(
            "UPDATE pipeline_jobs SET state = 'queued' \
             WHERE state = 'running' AND heartbeat_at < now() - interval '11 minutes' \
             AND attempts < $1",
        )
        .bind(MAX_ATTEMPTS)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Takes a queued run of an org with no run in progress and no erasure pending
    /// (a run started now could read rows about to be erased). The org served least
    /// recently goes first, so that one org's long queue does not hold the others.
    async fn claim(&self) -> Result<Option<(Uuid, i64, Uuid, i32, i32)>, sqlx::Error> {
        let claimed = sqlx::query(
            "UPDATE pipeline_jobs SET state = 'running', attempts = attempts + 1, \
             heartbeat_at = now(), started_at = coalesce(started_at, now()) \
             WHERE id = ( \
               SELECT j.id FROM pipeline_jobs j \
               WHERE j.state = 'queued' AND NOT EXISTS ( \
                 SELECT 1 FROM pipeline_jobs r WHERE r.org_id = j.org_id AND r.state = 'running') \
               AND NOT EXISTS ( \
                 SELECT 1 FROM dataset_erasures e WHERE e.org_id = j.org_id AND e.done_at IS NULL) \
               ORDER BY (SELECT max(l.started_at) FROM pipeline_jobs l WHERE l.org_id = j.org_id) \
                 NULLS FIRST, j.created_at \
               FOR UPDATE OF j SKIP LOCKED LIMIT 1 \
             ) RETURNING id, org_id, pipeline_id, version, attempts",
        )
        .fetch_optional(&self.pool)
        .await;
        let row = match claimed {
            Ok(row) => row,
            // Two workers took two runs of one org: the unique index stopped the
            // second, its run stays queued.
            Err(sqlx::Error::Database(e)) if e.is_unique_violation() => return Ok(None),
            Err(e) => return Err(e),
        };
        Ok(match row {
            Some(r) => Some((
                r.try_get("id")?,
                r.try_get("org_id")?,
                r.try_get("pipeline_id")?,
                r.try_get("version")?,
                r.try_get("attempts")?,
            )),
            None => None,
        })
    }

    /// Runs the next queued run. `false` when there is none.
    pub async fn run_next(&self) -> Result<bool, sqlx::Error> {
        self.sweep_dead().await?;
        // A person waits on a query: it goes before any run (analysis.md D827).
        if let Some(q) = self.claim_query().await? {
            self.run_query(q).await?;
            return Ok(true);
        }
        let Some((job, org_id, pipeline_id, version, attempts)) = self.claim().await? else {
            return Ok(false);
        };
        *self.current.lock().expect("heartbeat lock") = (Some(job), None);
        self.empty_spill_dir();
        let outcome = self.run_job(org_id, pipeline_id, version).await;
        *self.current.lock().expect("heartbeat lock") = (None, None);
        let (state, code, args) = match outcome {
            Ok(()) => ("done", None, None),
            Err(f) => {
                tracing::info!(job = %job, code = f.code, "pipeline run failed");
                ("failed", Some(f.code), Some(f.args))
            }
        };
        sqlx::query(
            "UPDATE pipeline_jobs SET state = $2, code = $3, args = $4, finished_at = now() \
             WHERE id = $1 AND state = 'running' AND attempts = $5",
        )
        .bind(job)
        .bind(state)
        .bind(code)
        .bind(args)
        // A late worker must not overwrite a run another one took back.
        .bind(attempts)
        .execute(&self.pool)
        .await?;
        Ok(true)
    }

    /// A killed run leaves its spill files (§9.7 bis). One run at a time per
    /// process, so whatever is here is dead.
    fn empty_spill_dir(&self) {
        let dir: &PathBuf = &self.config.limits.spill_dir;
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let removed = if path.is_dir() {
                std::fs::remove_dir_all(&path)
            } else {
                std::fs::remove_file(&path)
            };
            if let Err(e) = removed {
                tracing::warn!("spill file not removed: {e}");
            }
        }
    }

    async fn run_job(&self, org_id: i64, pipeline_id: Uuid, version: i32) -> Result<(), Failure> {
        let row = sqlx::query(
            "SELECT v.document FROM pipeline_versions v JOIN pipelines p ON p.id = v.pipeline_id \
             WHERE v.pipeline_id = $1 AND v.version = $2 AND p.org_id = $3",
        )
        .bind(pipeline_id)
        .bind(version)
        .bind(org_id)
        .fetch_one(&self.pool)
        .await
        .map_err(db_failure)?;
        let yaml: String = row.try_get("document").map_err(db_failure)?;
        // Validated when saved: a failure here is a document of an older format.
        let doc: PipelineDocument = serde_saphyr::from_str(&yaml)
            .map_err(|_| Failure::new(err_codes::PIPELINE_DOCUMENT_INVALID, Value::Null))?;
        let deadline = Instant::now() + self.config.run_timeout;
        for transform in &doc.transforms {
            self.run_transform(org_id, pipeline_id, version, transform, deadline)
                .await?;
        }
        Ok(())
    }

    /// The view of a dataset of the org: an input outside the org does not exist.
    async fn input(&self, org_id: i64, alias: &str, name: &str) -> Result<Input, Failure> {
        let missing = || {
            Failure::new(
                err_codes::PIPELINE_INPUT_MISSING,
                json!({ "dataset": name }),
            )
        };
        let dataset = sqlx::query("SELECT id FROM datasets WHERE org_id = $1 AND name = $2")
            .bind(org_id)
            .bind(name)
            .fetch_optional(&self.pool)
            .await
            .map_err(db_failure)?
            .ok_or_else(missing)?;
        let dataset_id: Uuid = dataset.try_get("id").map_err(db_failure)?;
        let rows = sqlx::query(
            "SELECT t.id, f.key FROM dataset_transactions t \
             JOIN dataset_files f ON f.transaction_id = t.id \
             WHERE t.dataset_id = $1 AND t.state = 'committed' AND t.purged_at IS NULL \
             AND t.seq >= coalesce(( \
               SELECT max(s.seq) FROM dataset_transactions s \
               WHERE s.dataset_id = $1 AND s.state = 'committed' AND s.kind = 'snapshot' \
             ), 0) ORDER BY t.seq, f.key",
        )
        .bind(dataset_id)
        .fetch_all(&self.pool)
        .await
        .map_err(db_failure)?;
        if rows.is_empty() {
            return Err(Failure::new(
                err_codes::PIPELINE_INPUT_EMPTY,
                json!({ "dataset": name }),
            ));
        }
        let (mut transactions, mut files) = (BTreeSet::new(), Vec::new());
        for r in &rows {
            transactions.insert(r.try_get::<Uuid, _>("id").map_err(db_failure)?);
            files.push(r.try_get::<String, _>("key").map_err(db_failure)?);
        }
        Ok(Input {
            alias: alias.to_string(),
            transactions,
            files,
        })
    }

    /// Same version, same inputs: the current output is already this result (D574).
    async fn up_to_date(
        &self,
        output_id: Uuid,
        pipeline_id: Uuid,
        version: i32,
        inputs: &BTreeSet<Uuid>,
    ) -> Result<bool, Failure> {
        let rows = sqlx::query(
            "SELECT i.input_transaction_id, i.pipeline_id, i.pipeline_version \
             FROM dataset_transaction_inputs i WHERE i.transaction_id = ( \
               SELECT t.id FROM dataset_transactions t \
               WHERE t.dataset_id = $1 AND t.state = 'committed' AND t.purged_at IS NULL \
               ORDER BY t.seq DESC LIMIT 1)",
        )
        .bind(output_id)
        .fetch_all(&self.pool)
        .await
        .map_err(db_failure)?;
        if rows.is_empty() {
            return Ok(false);
        }
        let mut read = BTreeSet::new();
        for r in &rows {
            // No pipeline: the one that computed it was deleted since.
            let (p, v): (Option<Uuid>, i32) = (
                r.try_get("pipeline_id").map_err(db_failure)?,
                r.try_get("pipeline_version").map_err(db_failure)?,
            );
            if p != Some(pipeline_id) || v != version {
                return Ok(false);
            }
            read.insert(
                r.try_get::<Uuid, _>("input_transaction_id")
                    .map_err(db_failure)?,
            );
        }
        Ok(&read == inputs)
    }

    /// The dataset an output writes, with the retention its transform sets (null: the
    /// default). The table of an import is refused: it feeds a sync written with the
    /// role of the import's author.
    async fn output_dataset(
        &self,
        org_id: i64,
        name: &str,
        retain_days: Option<u16>,
    ) -> Result<Uuid, Failure> {
        let row = sqlx::query(
            "INSERT INTO datasets (org_id, name, retain_days) VALUES ($1, $2, $3) \
             ON CONFLICT (org_id, name) DO UPDATE SET retain_days = EXCLUDED.retain_days \
             WHERE datasets.import_id IS NULL \
             RETURNING id, import_id",
        )
        .bind(org_id)
        .bind(name)
        .bind(retain_days.map(i32::from))
        .fetch_optional(&self.pool)
        .await
        .map_err(db_failure)?;
        // No row: the table of an import, left untouched.
        let Some(row) = row else {
            return Err(Failure::new(
                err_codes::DATASET_RESERVED,
                json!({ "dataset": name }),
            ));
        };
        let owner: Option<Uuid> = row.try_get("import_id").map_err(db_failure)?;
        if owner.is_some() {
            return Err(Failure::new(
                err_codes::DATASET_RESERVED,
                json!({ "dataset": name }),
            ));
        }
        row.try_get("id").map_err(db_failure)
    }

    async fn room(&self, org_id: i64) -> Result<u64, Failure> {
        let used: i64 = sqlx::query(
            "SELECT coalesce(sum(bytes), 0)::bigint AS b FROM dataset_transactions \
             WHERE org_id = $1 AND state = 'committed' AND purged_at IS NULL",
        )
        .bind(org_id)
        .fetch_one(&self.pool)
        .await
        .map_err(db_failure)?
        .try_get("b")
        .map_err(db_failure)?;
        QUOTA_BYTES
            .checked_sub(used.max(0) as u64)
            .filter(|n| *n > 0)
            .ok_or_else(|| Failure::new(err_codes::DATASET_QUOTA, json!({ "max": QUOTA_BYTES })))
    }

    async fn begin(&self, org_id: i64, output_id: Uuid) -> Result<Uuid, Failure> {
        sqlx::query(
            "UPDATE dataset_transactions SET state = 'aborted', code = $2 \
             WHERE dataset_id = $1 AND state = 'open' \
             AND heartbeat_at < now() - interval '10 minutes'",
        )
        .bind(output_id)
        .bind(err_codes::DATASET_TRANSACTION_ABORTED)
        .execute(&self.pool)
        .await
        .map_err(db_failure)?;
        let row = sqlx::query(
            "INSERT INTO dataset_transactions (org_id, dataset_id, kind) \
             VALUES ($1, $2, 'snapshot') \
             ON CONFLICT (dataset_id) WHERE state = 'open' DO NOTHING RETURNING id",
        )
        .bind(org_id)
        .bind(output_id)
        .fetch_optional(&self.pool)
        .await
        .map_err(db_failure)?
        .ok_or_else(|| Failure::new(err_codes::DATASET_TRANSACTION_OPEN, Value::Null))?;
        row.try_get("id").map_err(db_failure)
    }

    /// Fingerprints of the people erased from this output: what it computes from an
    /// input no one declared must not bring them back.
    async fn erased(&self, output_id: Uuid) -> Result<Erased, Failure> {
        let rows =
            sqlx::query("SELECT column_name, value FROM dataset_erasures WHERE dataset_id = $1")
                .bind(output_id)
                .fetch_all(&self.pool)
                .await
                .map_err(db_failure)?;
        let mut out = Erased::new();
        for r in &rows {
            out.entry(r.try_get("column_name").map_err(db_failure)?)
                .or_default()
                .insert(r.try_get("value").map_err(db_failure)?);
        }
        Ok(out)
    }

    async fn register_input(
        &self,
        ctx: &SessionContext,
        input: &Input,
    ) -> Result<(), DataFusionError> {
        let bucket = &self.config.s3_bucket;
        let paths: Vec<String> = input
            .files
            .iter()
            .map(|k| format!("s3://{bucket}/{k}"))
            .collect();
        let frame = ctx
            .read_parquet(paths, ParquetReadOptions::default())
            .await?;
        ctx.register_table(input.alias.as_str(), frame.into_view())?;
        Ok(())
    }

    /// A session holding the inputs of one transform.
    async fn session(
        &self,
        org_id: i64,
        inputs: &[Input],
        joins: engine::Joins,
    ) -> Result<SessionContext, DataFusionError> {
        let ctx = engine::session(&self.config.limits, joins)?;
        if let Some(master) = &self.config.pseudonym_key {
            engine::register_pseudonym(&ctx, engine::org_key(master, org_id));
        }
        let url = format!("s3://{}", self.config.s3_bucket)
            .parse::<url::Url>()
            .map_err(|e| DataFusionError::External(Box::new(e)))?;
        ctx.register_object_store(&url, self.store.clone());
        for input in inputs {
            self.register_input(&ctx, input).await?;
        }
        Ok(ctx)
    }

    async fn run_transform(
        &self,
        org_id: i64,
        pipeline_id: Uuid,
        version: i32,
        t: &Transform,
        deadline: Instant,
    ) -> Result<(), Failure> {
        let mut inputs = Vec::with_capacity(t.inputs.len());
        for (alias, name) in &t.inputs {
            inputs.push(self.input(org_id, alias, name).await?);
        }
        let read: BTreeSet<Uuid> = inputs
            .iter()
            .flat_map(|i| i.transactions.iter().copied())
            .collect();
        let output_id = self
            .output_dataset(org_id, &t.output, t.retain_days)
            .await?;
        if self
            .up_to_date(output_id, pipeline_id, version, &read)
            .await?
        {
            tracing::info!(output = %t.output, "pipeline output up to date");
            return Ok(());
        }
        let room = self.room(org_id).await?;

        let transaction = self.begin(org_id, output_id).await?;
        self.current.lock().expect("heartbeat lock").1 = Some(transaction);
        let key = format!("org_{org_id}/datasets/{output_id}/{transaction}/part-0.parquet");
        let erased = self.erased(output_id).await?;
        let timeout = self
            .config
            .transform_timeout
            .min(deadline.saturating_duration_since(Instant::now()));
        let write = async {
            // A join whose hash table outgrows the pool runs again sorting both
            // sides on disk (C6): slower, but it ends.
            let mut joins = engine::Joins::Hash;
            let written = loop {
                let ctx = self
                    .session(org_id, &inputs, joins)
                    .await
                    .map_err(|e| Failure::of_engine(&t.output, e))?;
                match self.write_query(&ctx, t, &key, room, &erased).await {
                    Err(f) if f.hash_join && joins == engine::Joins::Hash => {
                        tracing::info!(output = %t.output, "hash join over the pool, sorting instead");
                        joins = engine::Joins::SortMerge;
                    }
                    other => break other?,
                }
            };
            self.run_checks(transaction, output_id, t, &key, &written)
                .await?;
            Ok(written)
        };
        let result = match tokio::time::timeout(timeout, write).await {
            Ok(result) => result,
            Err(_) => Err(Failure::new(
                err_codes::PIPELINE_TIMEOUT,
                json!({ "output": t.output, "secs": timeout.as_secs() }),
            )),
        };
        let result = match result {
            Ok(written) => {
                self.commit(
                    pipeline_id,
                    version,
                    output_id,
                    transaction,
                    &key,
                    &written,
                    &read,
                )
                .await
            }
            Err(f) => Err(f),
        };
        self.current.lock().expect("heartbeat lock").1 = None;
        if let Err(f) = result {
            // The server's purge removes what this transaction wrote.
            let _ = sqlx::query(
                "UPDATE dataset_transactions SET state = 'aborted', code = $2 \
                 WHERE id = $1 AND state = 'open'",
            )
            .bind(transaction)
            .bind(f.code)
            .execute(&self.pool)
            .await;
            return Err(f);
        }
        Ok(())
    }

    /// D576: runs the checks on the file just written, keeps their results on the
    /// transaction, and refuses it if one fails. The caller aborts: the current
    /// version stays, and the purge removes this file.
    async fn run_checks(
        &self,
        transaction: Uuid,
        output_id: Uuid,
        t: &Transform,
        key: &str,
        written: &Written,
    ) -> Result<(), Failure> {
        if t.checks.is_empty() {
            return Ok(());
        }
        let results = self.measure(output_id, t, key, written).await?;
        sqlx::query("UPDATE dataset_transactions SET checks = $2 WHERE id = $1")
            .bind(transaction)
            .bind(json!(results))
            .execute(&self.pool)
            .await
            .map_err(db_failure)?;
        match results.iter().find(|r| !r.passed) {
            None => Ok(()),
            Some(r) => Err(Failure::new(
                err_codes::PIPELINE_CHECK_FAILED,
                json!({
                    "output": t.output,
                    "check": r.check,
                    "column": r.column,
                    "value": r.value,
                    "limit": r.limit,
                }),
            )),
        }
    }

    /// The version a check compares to: rows and columns, if there is one.
    async fn current_shape(&self, output_id: Uuid) -> Result<Option<(i64, Value)>, Failure> {
        let row = sqlx::query(
            "SELECT t.rows, d.schema FROM dataset_transactions t \
             JOIN datasets d ON d.id = t.dataset_id \
             WHERE t.dataset_id = $1 AND t.state = 'committed' AND t.purged_at IS NULL \
             ORDER BY t.seq DESC LIMIT 1",
        )
        .bind(output_id)
        .fetch_optional(&self.pool)
        .await
        .map_err(db_failure)?;
        row.map(|r| {
            Ok((
                r.try_get("rows").map_err(db_failure)?,
                r.try_get("schema").map_err(db_failure)?,
            ))
        })
        .transpose()
    }

    async fn measure(
        &self,
        output_id: Uuid,
        t: &Transform,
        key: &str,
        written: &Written,
    ) -> Result<Vec<CheckResult>, Failure> {
        let c = &t.checks;
        let output = t.output.as_str();
        let engine = |e| Failure::of_engine(output, e);
        let ctx = engine::session(&self.config.limits, engine::Joins::Hash).map_err(engine)?;
        let url = format!("s3://{}", self.config.s3_bucket)
            .parse::<url::Url>()
            .map_err(Failure::storage)?;
        ctx.register_object_store(&url, self.store.clone());
        let file = format!("s3://{}/{key}", self.config.s3_bucket);
        let frame = ctx
            .read_parquet(vec![file], ParquetReadOptions::default())
            .await
            .map_err(engine)?;
        ctx.register_table("result", frame.into_view())
            .map_err(engine)?;
        let columns: HashSet<&str> = written
            .schema
            .fields()
            .iter()
            .map(|f| f.name().as_str())
            .collect();
        let count = |sql: String| {
            let ctx = &ctx;
            async move {
                let frame = ctx
                    .sql_with_options(&sql, engine::read_only())
                    .await
                    .map_err(engine)?;
                let batches = frame.collect().await.map_err(engine)?;
                let n = batches
                    .iter()
                    .find(|b| b.num_rows() > 0)
                    .and_then(|b| {
                        cast(b.column(0), &DataType::Int64)
                            .ok()
                            .map(|a| a.as_primitive::<Int64Type>().value(0))
                    })
                    .unwrap_or(0);
                Ok::<i64, Failure>(n)
            }
        };
        let mut out = Vec::new();
        let rows = written.rows as i64;
        if let Some(min) = c.min_rows {
            out.push(check("min_rows", None, rows >= min as i64, rows, min));
        }
        if let Some(max) = c.max_rows {
            out.push(check("max_rows", None, rows <= max as i64, rows, max));
        }
        if !c.unique.is_empty() {
            let label = c.unique.join(", ");
            let n = if c.unique.iter().all(|k| columns.contains(k.as_str())) {
                count(duplicates_sql(&c.unique)).await?
            } else {
                // A column it no longer has identifies nothing.
                -1
            };
            out.push(check("unique", Some(label), n == 0, n, 0));
        }
        for (column, limit) in &c.max_empty_pct {
            let pct = if columns.contains(column.as_str()) {
                let empty = count(empty_sql(column)).await?;
                percent(empty, rows)
            } else {
                100
            };
            let passed = columns.contains(column.as_str()) && pct <= *limit as i64;
            out.push(check(
                "max_empty_pct",
                Some(column.clone()),
                passed,
                pct,
                limit,
            ));
        }
        if c.max_drop_pct.is_some() || c.same_columns {
            let current = self.current_shape(output_id).await?;
            if let (Some(limit), Some((before, _))) = (c.max_drop_pct, &current) {
                let drop = percent((before - rows).max(0), *before);
                out.push(check(
                    "max_drop_pct",
                    None,
                    drop <= limit as i64,
                    drop,
                    limit,
                ));
            }
            if let (true, Some((_, schema))) = (c.same_columns, &current) {
                let before: Vec<&str> = schema
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|f| f["name"].as_str())
                    .collect();
                let now: Vec<&str> = written
                    .schema
                    .fields()
                    .iter()
                    .map(|f| f.name().as_str())
                    .collect();
                out.push(CheckResult {
                    check: "same_columns".into(),
                    column: None,
                    passed: before == now,
                    value: now.join(", "),
                    limit: before.join(", "),
                });
            }
        }
        Ok(out)
    }

    async fn write_query(
        &self,
        ctx: &SessionContext,
        t: &Transform,
        key: &str,
        room: u64,
        erased: &Erased,
    ) -> Result<Written, Failure> {
        let output = t.output.as_str();
        let engine = |e| Failure::of_engine(output, e);
        let frame: DataFrame = ctx
            .sql_with_options(&t.sql, engine::read_only())
            .await
            .map_err(engine)?;
        let mut stream = frame.execute_stream().await.map_err(engine)?;
        let schema = stream.schema();
        let mut names = BTreeSet::new();
        for field in schema.fields() {
            if !names.insert(field.name().as_str()) {
                return Err(Failure::new(
                    err_codes::PIPELINE_SQL_INVALID,
                    json!({ "output": output, "detail": format!("duplicate column {}", field.name()) }),
                ));
            }
        }
        let properties = WriterProperties::builder()
            .set_compression(Compression::ZSTD(ZstdLevel::default()))
            .build();
        let sink = Sink {
            writer: BufWriter::new(self.store.clone(), StorePath::from(key)),
            hash: Sha256::new(),
            bytes: 0,
        };
        let mut writer = AsyncArrowWriter::try_new(sink, schema.clone(), Some(properties))
            .map_err(Failure::storage)?;
        let mut rows = 0u64;
        let out = async {
            while let Some(batch) = stream.next().await {
                let mut batch = batch.map_err(engine)?;
                if !erased.is_empty() {
                    batch = without(&batch, erased).map_err(Failure::storage)?;
                }
                rows += batch.num_rows() as u64;
                writer.write(&batch).await.map_err(Failure::storage)?;
                if writer.in_progress_size() > ROW_GROUP_BYTES {
                    writer.flush().await.map_err(Failure::storage)?;
                }
                if writer.bytes_written() as u64 > room {
                    return Err(Failure::new(
                        err_codes::DATASET_QUOTA,
                        json!({ "max": QUOTA_BYTES }),
                    ));
                }
            }
            writer.finish().await.map_err(Failure::storage)?;
            Ok(())
        }
        .await;
        let mut sink = writer.into_inner();
        if let Err(f) = out {
            if let Err(e) = sink.writer.abort().await {
                tracing::debug!("pipeline upload not aborted: {e}");
            }
            return Err(f);
        }
        Ok(Written {
            bytes: sink.bytes,
            rows,
            sha256: hex::encode(sink.hash.finalize()),
            schema,
        })
    }

    /// File, schema and lineage become visible together with the transaction.
    #[allow(clippy::too_many_arguments)]
    async fn commit(
        &self,
        pipeline_id: Uuid,
        version: i32,
        output_id: Uuid,
        transaction: Uuid,
        key: &str,
        written: &Written,
        read: &BTreeSet<Uuid>,
    ) -> Result<(), Failure> {
        let mut tx = self.pool.begin().await.map_err(db_failure)?;
        sqlx::query("SELECT id FROM datasets WHERE id = $1 FOR UPDATE")
            .bind(output_id)
            .execute(&mut *tx)
            .await
            .map_err(db_failure)?;
        let done = sqlx::query(
            "UPDATE dataset_transactions SET state = 'committed', \
             seq = nextval('dataset_commit_seq'), committed_at = now(), rows = $2, bytes = $3 \
             WHERE id = $1 AND state = 'open'",
        )
        .bind(transaction)
        .bind(written.rows as i64)
        .bind(written.bytes as i64)
        .execute(&mut *tx)
        .await
        .map_err(db_failure)?;
        if done.rows_affected() == 0 {
            return Err(Failure::new(
                err_codes::DATASET_TRANSACTION_ABORTED,
                Value::Null,
            ));
        }
        sqlx::query(
            "INSERT INTO dataset_files (transaction_id, key, bytes, rows, sha256) \
             VALUES ($1, $2, $3, $4, $5)",
        )
        .bind(transaction)
        .bind(key)
        .bind(written.bytes as i64)
        .bind(written.rows as i64)
        .bind(&written.sha256)
        .execute(&mut *tx)
        .await
        .map_err(db_failure)?;
        let columns: Vec<Value> = written
            .schema
            .fields()
            .iter()
            .map(|f| json!({ "name": f.name(), "data_type": type_name(f.data_type()) }))
            .collect();
        sqlx::query("UPDATE datasets SET schema = $2 WHERE id = $1")
            .bind(output_id)
            .bind(Value::Array(columns))
            .execute(&mut *tx)
            .await
            .map_err(db_failure)?;
        for input in read {
            sqlx::query(
                "INSERT INTO dataset_transaction_inputs \
                 (transaction_id, input_transaction_id, pipeline_id, pipeline_version) \
                 VALUES ($1, $2, $3, $4)",
            )
            .bind(transaction)
            .bind(input)
            .bind(pipeline_id)
            .bind(version)
            .execute(&mut *tx)
            .await
            .map_err(db_failure)?;
        }
        tx.commit().await.map_err(db_failure)
    }
}

fn check(
    name: &str,
    column: Option<String>,
    passed: bool,
    value: impl std::fmt::Display,
    limit: impl std::fmt::Display,
) -> CheckResult {
    CheckResult {
        check: name.into(),
        column,
        passed,
        value: value.to_string(),
        limit: limit.to_string(),
    }
}

/// The share, rounded up: 1 row in a million is not 0 %.
fn percent(part: i64, whole: i64) -> i64 {
    if whole <= 0 {
        return 0;
    }
    (part.saturating_mul(100) + whole - 1) / whole
}

/// Column names come from the document: quoted, and only asked about once found in
/// the schema of the result.
fn quoted(column: &str) -> String {
    format!("\"{}\"", column.replace('"', "\"\""))
}

fn duplicates_sql(columns: &[String]) -> String {
    let keys = columns
        .iter()
        .map(|c| quoted(c))
        .collect::<Vec<_>>()
        .join(", ");
    format!("SELECT count(*) FROM (SELECT 1 FROM result GROUP BY {keys} HAVING count(*) > 1) d")
}

fn empty_sql(column: &str) -> String {
    let c = quoted(column);
    format!(
        "SELECT coalesce(sum(CASE WHEN {c} IS NULL OR btrim(CAST({c} AS VARCHAR)) = '' \
         THEN 1 ELSE 0 END), 0) FROM result"
    )
}

/// Column → fingerprints of the values to leave out.
type Erased = HashMap<String, HashSet<String>>;

/// A batch without the rows of erased people. Same rule as the server's
/// `datasets::parquet::matches`, on the arrow version of the engine.
fn without(batch: &RecordBatch, erased: &Erased) -> Result<RecordBatch, ArrowError> {
    let mut keep = vec![true; batch.num_rows()];
    for (column, values) in erased {
        let Some(array) = batch.column_by_name(column) else {
            continue;
        };
        if !can_cast_types(array.data_type(), &DataType::Utf8) {
            continue;
        }
        let text = cast(array, &DataType::Utf8)?;
        for (i, value) in text.as_string::<i32>().iter().enumerate() {
            let print = value.and_then(pnex_core::datasets::fingerprint);
            if print.is_some_and(|v| values.contains(&v)) {
                keep[i] = false;
            }
        }
    }
    filter_record_batch(batch, &BooleanArray::from(keep))
}

/// What a query wrote.
struct Written {
    bytes: u64,
    rows: u64,
    sha256: String,
    schema: SchemaRef,
}

/// The type a column shows in the catalogue.
fn type_name(t: &DataType) -> String {
    match t {
        DataType::Utf8 | DataType::LargeUtf8 | DataType::Utf8View => "text".into(),
        DataType::Date32 | DataType::Date64 => "date".into(),
        DataType::Timestamp(..) => "timestamp".into(),
        DataType::Decimal128(..) | DataType::Decimal256(..) => "decimal".into(),
        other => format!("{other:?}").to_lowercase(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_share_rounds_up_and_survives_nothing() {
        assert_eq!(percent(0, 0), 0);
        assert_eq!(percent(1, 1_000_000), 1);
        assert_eq!(percent(1, 2), 50);
        assert_eq!(percent(3, 3), 100);
    }

    #[test]
    fn a_column_name_cannot_leave_its_quotes() {
        assert_eq!(quoted(r#"a"b"#), r#""a""b""#);
        assert!(duplicates_sql(&["x".into(), r#"y"; DROP"#.into()]).contains(r#""y""; DROP""#));
    }
}
