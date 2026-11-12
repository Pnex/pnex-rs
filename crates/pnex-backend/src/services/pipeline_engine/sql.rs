//! The `sql` step (pipeline-engine.md D936, D943): the working tables a
//! query names are staged as Parquet, the pipeline worker runs it, and its
//! result comes back as a working table. User SQL never runs in the server.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use futures_util::TryStreamExt;
use parquet::arrow::AsyncArrowWriter;
use parquet::basic::{Compression, ZstdLevel};
use parquet::file::properties::WriterProperties;
use sea_orm::{ConnectionTrait, DatabaseConnection, Statement};
use serde_json::{json, Value};
use uuid::Uuid;

use super::blob::{BatchWriter, Batches};
use super::{fail, Failure};
use crate::services::artifact_store::S3Store;
use crate::services::datasets::parquet::{self as pq, S3Sink};

/// How long the server waits on the worker for one step.
const WAIT: Duration = Duration::from_secs(15 * 60);
const POLL: Duration = Duration::from_millis(500);

/// Where a query runs.
#[allow(async_fn_in_trait)]
pub trait SqlRunner {
    /// `staged`: working tables by SQL name; `tables`: SQL name → table of
    /// the org. `None` when the query gave no row.
    async fn run(
        &self,
        sql: &str,
        staged: Vec<(String, Arc<Batches>)>,
        tables: &BTreeMap<String, String>,
    ) -> Result<Option<Arc<Batches>>, Failure>;

    /// A `call`: Starlark `code` over the rows of `input` (none: an empty
    /// list), with the pack settings it may read.
    async fn call(
        &self,
        code: &str,
        settings: &serde_json::Map<String, Value>,
        input: Option<Arc<Batches>>,
    ) -> Result<Option<Arc<Batches>>, Failure>;
}

fn storage(e: impl std::fmt::Display) -> Failure {
    tracing::warn!(error = %e, "pipeline step staging failed");
    fail("storage", json!({ "what": "step" }))
}

/// The pipeline worker, through its queue.
pub struct WorkerSql<'a> {
    pub db: &'a DatabaseConnection,
    pub store: &'a S3Store,
    pub org: i64,
}

impl WorkerSql<'_> {
    async fn stage(&self, key: &str, file: &Batches) -> Result<(), Failure> {
        let upload = self.store.writer(key).await.map_err(storage)?;
        let properties = WriterProperties::builder()
            .set_compression(Compression::ZSTD(ZstdLevel::default()))
            .build();
        let mut writer =
            AsyncArrowWriter::try_new(S3Sink::new(upload), file.schema.clone(), Some(properties))
                .map_err(storage)?;
        for b in file.read()? {
            writer.write(&b?).await.map_err(storage)?;
        }
        writer.close().await.map_err(storage)?;
        Ok(())
    }

    async fn wait(&self, id: Uuid) -> Result<(String, Option<Value>, Option<String>), Failure> {
        let started = Instant::now();
        loop {
            let row = self
                .db
                .query_one_raw(Statement::from_sql_and_values(
                    sea_orm::DatabaseBackend::Postgres,
                    "SELECT state, code, args FROM pipeline_step_jobs WHERE id = $1",
                    vec![id.into()],
                ))
                .await
                .map_err(storage)?
                .ok_or_else(|| fail("storage", json!({ "what": "step" })))?;
            let state: String = row.try_get("", "state").map_err(storage)?;
            if state == "done" || state == "failed" {
                let code: Option<String> = row.try_get("", "code").map_err(storage)?;
                let args: Option<Value> = row.try_get("", "args").map_err(storage)?;
                return Ok((state, args, code));
            }
            if started.elapsed() > WAIT {
                return Err(fail("sql_timeout", Value::Null));
            }
            tokio::time::sleep(POLL).await;
        }
    }
}

impl SqlRunner for WorkerSql<'_> {
    async fn call(
        &self,
        code: &str,
        settings: &serde_json::Map<String, Value>,
        input: Option<Arc<Batches>>,
    ) -> Result<Option<Arc<Batches>>, Failure> {
        let staged = input
            .map(|f| ("input".to_string(), f))
            .into_iter()
            .collect();
        self.queue("call", code, settings, staged, &BTreeMap::new())
            .await
    }

    async fn run(
        &self,
        sql: &str,
        staged: Vec<(String, Arc<Batches>)>,
        tables: &BTreeMap<String, String>,
    ) -> Result<Option<Arc<Batches>>, Failure> {
        self.queue("sql", sql, &serde_json::Map::new(), staged, tables)
            .await
    }
}

impl WorkerSql<'_> {
    /// Stages the working tables, queues the step and reads its result back.
    async fn queue(
        &self,
        kind: &str,
        text: &str,
        settings: &serde_json::Map<String, Value>,
        staged: Vec<(String, Arc<Batches>)>,
        tables: &BTreeMap<String, String>,
    ) -> Result<Option<Arc<Batches>>, Failure> {
        let sql = text;
        let id = Uuid::new_v4();
        // The org's prefix first (R18), as its staging.
        let prefix = format!("org_{}/pipeline-steps/{id}/", self.org);
        let mut keys = Vec::new();
        let mut staged_keys = serde_json::Map::new();
        let outcome = async {
            for (alias, file) in &staged {
                let key = format!("{prefix}{alias}.parquet");
                keys.push(key.clone());
                self.stage(&key, file).await?;
                staged_keys.insert(alias.clone(), Value::String(key));
            }
            let output = format!("{prefix}out.parquet");
            keys.push(output.clone());
            self.db
                .execute_raw(Statement::from_sql_and_values(
                    sea_orm::DatabaseBackend::Postgres,
                    "INSERT INTO pipeline_step_jobs (id, org_id, sql, staged, tables, output_key, \
                       kind, settings) VALUES ($1, $2, $3, $4, $5, $6, $7, $8)",
                    vec![
                        id.into(),
                        self.org.into(),
                        sql.into(),
                        Value::Object(staged_keys.clone()).into(),
                        json!(tables).into(),
                        output.clone().into(),
                        kind.into(),
                        Value::Object(settings.clone()).into(),
                    ],
                ))
                .await
                .map_err(storage)?;
            let (state, args, code) = self.wait(id).await?;
            if state == "failed" {
                return Err(fail("sql_failed", json!({ "code": code, "args": args })));
            }
            let Some(size) = self.store.size(&output).await.map_err(storage)? else {
                return Ok(None);
            };
            let mut stream = pq::batches(self.store, &output, size)
                .await
                .map_err(storage)?;
            let mut writer: Option<BatchWriter> = None;
            while let Some(b) = stream.try_next().await.map_err(storage)? {
                if writer.is_none() {
                    writer = Some(BatchWriter::new(b.schema())?);
                }
                if let Some(w) = &mut writer {
                    w.write(&b)?;
                }
            }
            writer.map(BatchWriter::finish).transpose()
        }
        .await;
        for key in keys {
            if let Err(e) = self.store.delete_key(&key).await {
                tracing::debug!(key, error = %e, "pipeline step file not deleted");
            }
        }
        outcome
    }
}
