//! The SQL of data pipelines (pipelines.md D943, D934) and of analysis
//! queries (D827): a queue in PostgreSQL, DataFusion over the org's tables.
//! The server runs the pipeline and waits on each step here. This process
//! holds no secret but its catalogue and S3 access, and sees no business
//! table.

mod analysis;
pub mod engine;
pub mod step;
pub mod tz;

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use datafusion::error::DataFusionError;
use datafusion::parquet::arrow::async_writer::AsyncFileWriter;
use datafusion::parquet::errors::{ParquetError, Result as ParquetResult};
use datafusion::prelude::{ParquetReadOptions, SessionContext};
use futures::future::BoxFuture;
use futures::FutureExt;
use object_store::aws::{AmazonS3, AmazonS3Builder};
use object_store::buffered::BufWriter;
use pnex_core::err_codes;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use sqlx::postgres::{PgConnectOptions, PgPoolOptions};
use sqlx::{PgPool, Row};
use tokio::io::AsyncWriteExt;
use uuid::Uuid;

use self::engine::Limits;

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
    files: Vec<String>,
}

pub struct Worker {
    pool: PgPool,
    store: Arc<AmazonS3>,
    config: Config,
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
        let worker = Self {
            pool,
            store: Arc::new(store),
            config,
        };
        Ok(worker)
    }

    /// Runs the next queued run. `false` when there is none.
    pub async fn run_next(&self) -> Result<bool, sqlx::Error> {
        // A person waits on a query: it goes before any run (analysis.md D827).
        if let Some(q) = self.claim_query().await? {
            self.run_query(q).await?;
            return Ok(true);
        }
        // A pipeline step holds a server run waiting on it.
        if let Some(s) = self.claim_step().await? {
            self.run_step(s).await?;
            return Ok(true);
        }
        Ok(false)
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
        let mut files = Vec::new();
        for r in &rows {
            files.push(r.try_get::<String, _>("key").map_err(db_failure)?);
        }
        Ok(Input {
            alias: alias.to_string(),
            files,
        })
    }

    /// A session holding the inputs of one query.
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
}
