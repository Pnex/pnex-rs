//! Read-only SQL queries (analysis.md D825–D827), taken before pipeline runs. The
//! server fixed what a query may read; this module registers only that, runs the
//! query with the transforms' guards plus a deadline, and keeps its first rows.

use std::sync::Arc;
use std::time::Duration;

use datafusion::arrow::array::{cast::AsArray, Array};
use datafusion::arrow::compute::{can_cast_types, cast};
use datafusion::arrow::datatypes::DataType;
use datafusion::catalog::MemorySchemaProvider;
use datafusion::error::DataFusionError;
use pnex_core::analysis::{OBJECTS_SCHEMA, ROWS_MAX};
use pnex_core::err_codes;
use serde_json::{json, Map, Value};
use sqlx::Row;
use uuid::Uuid;

use super::{cut, engine, Failure, Input, Worker};

/// D826: how long a query may run, and how heavy its rows may be.
const QUERY_TIMEOUT: Duration = Duration::from_secs(60);
const RESULT_MAX_BYTES: usize = 10 * 1024 * 1024;

/// A query the server queued, with what it may read.
pub(crate) struct Queued {
    pub id: Uuid,
    pub org_id: i64,
    pub sql: String,
    pub tables: Map<String, Value>,
    pub refused: Vec<String>,
    pub objects: Map<String, Value>,
    pub attempts: i32,
}

fn sql_invalid(e: &DataFusionError) -> Failure {
    Failure::new(
        err_codes::ANALYSIS_SQL_INVALID,
        json!({ "reason": cut(&e.find_root().to_string()) }),
    )
}

impl Worker {
    /// The oldest queued query, now running.
    pub(crate) async fn claim_query(&self) -> Result<Option<Queued>, sqlx::Error> {
        // A worker killed mid-query would block its author for good (one query each).
        sqlx::query(
            "UPDATE analysis_queries SET state = 'failed', code = $1, finished_at = now() \
             WHERE state = 'running' AND started_at < now() - interval '5 minutes'",
        )
        .bind(err_codes::ANALYSIS_TIMEOUT)
        .execute(&self.pool)
        .await?;
        let row = sqlx::query(
            "UPDATE analysis_queries SET state = 'running', attempts = attempts + 1, \
               started_at = now() \
             WHERE id = (SELECT id FROM analysis_queries WHERE state = 'queued' \
               ORDER BY created_at LIMIT 1 FOR UPDATE SKIP LOCKED) \
             RETURNING id, org_id, sql, tables, refused, objects, attempts",
        )
        .fetch_optional(&self.pool)
        .await?;
        let Some(r) = row else {
            return Ok(None);
        };
        let obj = |v: Value| v.as_object().cloned().unwrap_or_default();
        Ok(Some(Queued {
            id: r.try_get("id")?,
            org_id: r.try_get("org_id")?,
            sql: r.try_get("sql")?,
            tables: obj(r.try_get("tables")?),
            refused: serde_json::from_value(r.try_get("refused")?).unwrap_or_default(),
            objects: obj(r.try_get("objects")?),
            attempts: r.try_get("attempts")?,
        }))
    }

    /// Runs one query and records its rows or why it stopped.
    pub(crate) async fn run_query(&self, q: Queued) -> Result<(), sqlx::Error> {
        self.empty_spill_dir();
        let outcome = self.query_rows(&q).await;
        let (state, result, code, args) = match outcome {
            Ok(result) => ("done", Some(result), None, None),
            Err(f) => {
                tracing::info!(query = %q.id, code = f.code, "analysis query failed");
                ("failed", None, Some(f.code), Some(f.args))
            }
        };
        sqlx::query(
            "UPDATE analysis_queries SET state = $2, result = $3, code = $4, args = $5, \
               finished_at = now() WHERE id = $1 AND state = 'running' AND attempts = $6",
        )
        .bind(q.id)
        .bind(state)
        .bind(result)
        .bind(code)
        .bind(args)
        .bind(q.attempts)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn query_rows(&self, q: &Queued) -> Result<Value, Failure> {
        let ctx = self
            .session(q.org_id, &[], engine::Joins::Hash)
            .await
            .map_err(Failure::storage)?;
        let objects = Arc::new(MemorySchemaProvider::new());
        if let Some(catalog) = ctx.catalog("datafusion") {
            let _ = catalog.register_schema(OBJECTS_SCHEMA, objects);
        }
        let state = ctx.state();
        let statement = state
            .sql_to_statement(&q.sql, &Default::default())
            .map_err(|e| sql_invalid(&e))?;
        let refs = state
            .resolve_table_references(&statement)
            .map_err(|e| sql_invalid(&e))?;
        for r in refs {
            let table = r.table().to_string();
            let unknown = || {
                Failure::new(
                    err_codes::ANALYSIS_TABLE_UNKNOWN,
                    json!({ "table": r.to_string() }),
                )
            };
            let input = match r.schema() {
                None if q.refused.contains(&table) => {
                    return Err(Failure::new(
                        err_codes::MARKING_REQUIRED,
                        json!({ "table": table }),
                    ))
                }
                None if q.tables.contains_key(&table) => {
                    self.input(q.org_id, &table, &table).await?
                }
                Some(OBJECTS_SCHEMA) => {
                    let key = q
                        .objects
                        .get(&table)
                        .and_then(Value::as_str)
                        .ok_or_else(unknown)?;
                    Input {
                        alias: format!("{OBJECTS_SCHEMA}.{table}"),
                        transactions: Default::default(),
                        files: vec![key.to_string()],
                    }
                }
                _ => return Err(unknown()),
            };
            self.register_input(&ctx, &input)
                .await
                .map_err(Failure::storage)?;
        }
        let run = async {
            let frame = ctx.sql_with_options(&q.sql, engine::read_only()).await?;
            frame.limit(0, Some(ROWS_MAX + 1))?.collect().await
        };
        let batches = match tokio::time::timeout(QUERY_TIMEOUT, run).await {
            Err(_) => return Err(Failure::new(err_codes::ANALYSIS_TIMEOUT, Value::Null)),
            Ok(Err(e)) => return Err(query_failure(e)),
            Ok(Ok(b)) => b,
        };
        rows_of(batches)
    }
}

/// An error met while running: only one about the query itself is shown.
fn query_failure(e: DataFusionError) -> Failure {
    match e.find_root() {
        DataFusionError::ResourcesExhausted(_) => {
            Failure::new(err_codes::PIPELINE_MEMORY_EXCEEDED, Value::Null)
        }
        DataFusionError::ObjectStore(_)
        | DataFusionError::IoError(_)
        | DataFusionError::ParquetError(_)
        | DataFusionError::External(_) => Failure::storage(&e),
        _ => sql_invalid(&e),
    }
}

/// The rows as text, at most [`ROWS_MAX`], `truncated` past it.
fn rows_of(batches: Vec<datafusion::arrow::array::RecordBatch>) -> Result<Value, Failure> {
    let columns: Vec<String> = batches
        .first()
        .map(|b| {
            b.schema()
                .fields()
                .iter()
                .map(|f| f.name().clone())
                .collect()
        })
        .unwrap_or_default();
    let mut rows: Vec<Vec<Option<String>>> = Vec::new();
    'all: for b in &batches {
        let text: Vec<Option<Arc<dyn Array>>> = b
            .columns()
            .iter()
            .map(|c| {
                can_cast_types(c.data_type(), &DataType::Utf8)
                    .then(|| cast(c, &DataType::Utf8).ok())
                    .flatten()
            })
            .collect();
        for i in 0..b.num_rows() {
            if rows.len() == ROWS_MAX {
                break 'all;
            }
            rows.push(
                text.iter()
                    .map(|c| {
                        let a = c.as_ref()?.as_string::<i32>();
                        (!a.is_null(i)).then(|| a.value(i).to_string())
                    })
                    .collect(),
            );
        }
    }
    let total: usize = batches.iter().map(|b| b.num_rows()).sum();
    let result = json!({ "columns": columns, "rows": rows, "truncated": total > ROWS_MAX });
    if result.to_string().len() > RESULT_MAX_BYTES {
        return Err(Failure::new(
            err_codes::ANALYSIS_RESULT_TOO_LARGE,
            Value::Null,
        ));
    }
    Ok(result)
}
