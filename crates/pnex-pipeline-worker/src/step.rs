//! The `sql` step of a pipeline (pipeline-engine.md D936, D943). The server
//! stages the working tables a query reads and queues it; this runs it with
//! the transforms' session, allow list and limits, and writes the result
//! back as one Parquet file. User SQL stays out of the server.

use std::time::Duration;

use datafusion::arrow::array::RecordBatch;
use datafusion::arrow::datatypes::SchemaRef;
use datafusion::datasource::MemTable;
use datafusion::parquet::arrow::async_writer::AsyncArrowWriter;
use datafusion::parquet::basic::{Compression, ZstdLevel};
use datafusion::parquet::file::properties::WriterProperties;
use datafusion::prelude::{ParquetReadOptions, SessionContext};
use futures::StreamExt;
use object_store::buffered::BufWriter;
use object_store::path::Path as StorePath;
use pnex_core::err_codes;
use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};
use sqlx::Row;
use uuid::Uuid;

use super::{cut, engine, Failure, Sink, Worker};

/// How long one step may run.
const STEP_TIMEOUT: Duration = Duration::from_secs(10 * 60);
/// Largest result of one step.
const RESULT_MAX_BYTES: u64 = 4 * 1024 * 1024 * 1024;

/// A step the server queued.
pub(crate) struct Queued {
    pub id: Uuid,
    pub org_id: i64,
    pub sql: String,
    pub staged: Map<String, Value>,
    pub tables: Map<String, Value>,
    pub output_key: String,
    pub attempts: i32,
}

fn invalid(e: &datafusion::error::DataFusionError) -> Failure {
    Failure::new(
        err_codes::PIPELINE_SQL_INVALID,
        json!({ "reason": cut(&e.find_root().to_string()) }),
    )
}

/// Runs `sql` over in-memory tables, with the session of a transform: what
/// the replay tests of the shipped pipelines run (D945).
pub async fn run_in_memory(
    limits: &engine::Limits,
    sql: &str,
    tables: Vec<(String, SchemaRef, Vec<RecordBatch>)>,
) -> Result<Vec<RecordBatch>, String> {
    let ctx = engine::session(limits, engine::Joins::Hash).map_err(|e| e.to_string())?;
    for (name, schema, batches) in tables {
        let t = MemTable::try_new(schema, vec![batches]).map_err(|e| e.to_string())?;
        ctx.register_table(name.as_str(), std::sync::Arc::new(t))
            .map_err(|e| e.to_string())?;
    }
    ctx.sql_with_options(sql, engine::read_only())
        .await
        .map_err(|e| e.find_root().to_string())?
        .collect()
        .await
        .map_err(|e| e.find_root().to_string())
}

impl Worker {
    /// The oldest queued step, now running.
    pub(crate) async fn claim_step(&self) -> Result<Option<Queued>, sqlx::Error> {
        // A worker killed mid-step leaves it running: failed after its deadline.
        sqlx::query(
            "UPDATE pipeline_step_jobs SET state = 'failed', code = $1, finished_at = now() \
             WHERE state = 'running' AND started_at < now() - interval '15 minutes'",
        )
        .bind(err_codes::PIPELINE_TIMEOUT)
        .execute(&self.pool)
        .await?;
        let row = sqlx::query(
            "UPDATE pipeline_step_jobs SET state = 'running', attempts = attempts + 1, \
               started_at = now() \
             WHERE id = (SELECT id FROM pipeline_step_jobs WHERE state = 'queued' \
               ORDER BY created_at LIMIT 1 FOR UPDATE SKIP LOCKED) \
             RETURNING id, org_id, sql, staged, tables, output_key, attempts",
        )
        .fetch_optional(&self.pool)
        .await?;
        let Some(r) = row else { return Ok(None) };
        let obj = |v: Value| v.as_object().cloned().unwrap_or_default();
        Ok(Some(Queued {
            id: r.try_get("id")?,
            org_id: r.try_get("org_id")?,
            sql: r.try_get("sql")?,
            staged: obj(r.try_get("staged")?),
            tables: obj(r.try_get("tables")?),
            output_key: r.try_get("output_key")?,
            attempts: r.try_get("attempts")?,
        }))
    }

    /// Runs one step and records its rows or why it stopped.
    pub(crate) async fn run_step(&self, q: Queued) -> Result<(), sqlx::Error> {
        self.empty_spill_dir();
        let outcome = tokio::time::timeout(STEP_TIMEOUT, self.step_rows(&q))
            .await
            .unwrap_or_else(|_| Err(Failure::new(err_codes::PIPELINE_TIMEOUT, Value::Null)));
        let (state, rows, code, args) = match outcome {
            Ok(rows) => ("done", Some(rows as i64), None, None),
            Err(f) => {
                tracing::info!(step = %q.id, code = f.code, "pipeline step failed");
                ("failed", None, Some(f.code), Some(f.args))
            }
        };
        sqlx::query(
            "UPDATE pipeline_step_jobs SET state = $2, rows = $3, code = $4, args = $5, \
               finished_at = now() WHERE id = $1 AND state = 'running' AND attempts = $6",
        )
        .bind(q.id)
        .bind(state)
        .bind(rows)
        .bind(code)
        .bind(args)
        .bind(q.attempts)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn step_rows(&self, q: &Queued) -> Result<u64, Failure> {
        let mut inputs = Vec::new();
        for (alias, name) in &q.tables {
            let name = name.as_str().unwrap_or_default();
            inputs.push(self.input(q.org_id, alias, name).await?);
        }
        let ctx: SessionContext = self
            .session(q.org_id, &inputs, engine::Joins::Hash)
            .await
            .map_err(Failure::storage)?;
        let bucket = &self.config.s3_bucket;
        for (alias, key) in &q.staged {
            let key = key.as_str().unwrap_or_default();
            let frame = ctx
                .read_parquet(
                    format!("s3://{bucket}/{key}"),
                    ParquetReadOptions::default(),
                )
                .await
                .map_err(Failure::storage)?;
            ctx.register_table(alias.as_str(), frame.into_view())
                .map_err(Failure::storage)?;
        }
        let frame = ctx
            .sql_with_options(&q.sql, engine::read_only())
            .await
            .map_err(|e| invalid(&e))?;
        let mut stream = frame.execute_stream().await.map_err(|e| invalid(&e))?;
        let properties = WriterProperties::builder()
            .set_compression(Compression::ZSTD(ZstdLevel::default()))
            .build();
        let sink = Sink {
            writer: BufWriter::new(self.store.clone(), StorePath::from(q.output_key.as_str())),
            hash: Sha256::new(),
            bytes: 0,
        };
        let mut writer = AsyncArrowWriter::try_new(sink, stream.schema(), Some(properties))
            .map_err(Failure::storage)?;
        let mut rows = 0u64;
        let out = async {
            while let Some(batch) = stream.next().await {
                let batch = batch.map_err(|e| invalid(&e))?;
                rows += batch.num_rows() as u64;
                writer.write(&batch).await.map_err(Failure::storage)?;
                if writer.bytes_written() as u64 > RESULT_MAX_BYTES {
                    return Err(Failure::new(
                        err_codes::DATASET_QUOTA,
                        json!({ "max": RESULT_MAX_BYTES }),
                    ));
                }
            }
            writer.finish().await.map_err(Failure::storage)?;
            Ok(())
        }
        .await;
        if let Err(f) = out {
            if let Err(e) = writer.into_inner().writer.abort().await {
                tracing::debug!("pipeline step upload not aborted: {e}");
            }
            return Err(f);
        }
        Ok(rows)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use datafusion::arrow::array::{Float64Array, StringArray};
    use datafusion::arrow::datatypes::{DataType, Field, Schema};
    use std::sync::Arc;

    /// One record into several metrics, a derived one: the COT shape in SQL.
    #[tokio::test]
    async fn a_wide_record_becomes_long_rows() {
        let schema = Arc::new(Schema::new(vec![
            Field::new("day", DataType::Utf8, true),
            Field::new("long", DataType::Float64, true),
            Field::new("short", DataType::Float64, true),
        ]));
        let b = RecordBatch::try_new(
            schema,
            vec![
                Arc::new(StringArray::from(vec!["2024-03-05"])),
                Arc::new(Float64Array::from(vec![10.0])),
                Arc::new(Float64Array::from(vec![4.0])),
            ],
        )
        .unwrap();
        let limits = engine::Limits {
            memory_bytes: 64 * 1024 * 1024,
            partitions: 1,
            spill_dir: std::env::temp_dir(),
            spill_max_bytes: 1024 * 1024 * 1024,
        };
        let sql = "SELECT day, 'long' AS metric, long AS value FROM r \
                   UNION ALL SELECT day, 'net', long - short FROM r ORDER BY metric";
        let out = run_in_memory(&limits, sql, vec![("r".into(), b.schema(), vec![b])])
            .await
            .unwrap();
        assert_eq!(out.iter().map(RecordBatch::num_rows).sum::<usize>(), 2);
        let err = run_in_memory(&limits, "DROP TABLE r", vec![]).await;
        assert!(err.is_err());
    }
}
