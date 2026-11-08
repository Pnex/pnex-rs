//! Read-only SQL (analysis.md D825–D828). The server fixes what a query may read
//! when it is sent: the tables of the org its author reads, the marked ones he is
//! not cleared for named as refused, and each `objects.<type>` frozen as the author
//! sees it. The worker runs it and never sees more.

use std::sync::Arc;

use arrow_array::{ArrayRef, RecordBatch, StringArray};
use arrow_schema::{DataType, Field, Schema};
use pnex_core::analysis::{
    object_types, AnalysisQuery, AnalysisState, OBJECT_TYPES_MAX, REUSE_MAX_SECS, SQL_MAX_BYTES,
};
use pnex_core::err_codes;
use sea_orm::{
    ConnectionTrait, DatabaseBackend, DatabaseConnection, DbErr, QueryResult, Statement,
};
use serde_json::{json, Value};
use uuid::Uuid;

use crate::services::artifact_store::S3Store;
use crate::services::ontology::redact::Redaction;

/// Objects one type may freeze for a query.
pub const OBJECTS_MAX: u64 = 50_000;

#[derive(Debug)]
pub enum AnalysisError {
    Invalid {
        field: &'static str,
        token: String,
    },
    Busy,
    NotFound,
    /// A machine code with its args (too many objects, storage).
    Coded(&'static str, Value),
    Db(DbErr),
}

impl From<DbErr> for AnalysisError {
    fn from(e: DbErr) -> Self {
        Self::Db(e)
    }
}

fn stmt(sql: &str, values: Vec<sea_orm::Value>) -> Statement {
    Statement::from_sql_and_values(DatabaseBackend::Postgres, sql, values)
}

/// The prefix of a query's frozen objects.
fn prefix(org_id: i64, id: Uuid) -> String {
    format!("org_{org_id}/analysis/{id}/")
}

/// Sends a query (D825): checks, fixes what it may read, queues it.
pub async fn submit(
    db: &DatabaseConnection,
    store: &S3Store,
    org_id: i64,
    user: i64,
    hide: Redaction,
    sql: &str,
    max_age_secs: Option<u32>,
) -> Result<AnalysisQuery, AnalysisError> {
    let sql = sql.trim();
    if sql.is_empty() {
        return Err(AnalysisError::Invalid {
            field: "sql",
            token: "required".into(),
        });
    }
    if sql.len() > SQL_MAX_BYTES {
        return Err(AnalysisError::Invalid {
            field: "sql",
            token: format!("max_length:{SQL_MAX_BYTES}"),
        });
    }
    if let Some(age) = max_age_secs.map(|a| a.min(REUSE_MAX_SECS)) {
        let earlier = db
            .query_one_raw(stmt(
                "SELECT id FROM analysis_queries WHERE org_id = $1 AND user_id = $2 AND sql = $3 \
                 AND (state IN ('queued', 'running') \
                   OR (state = 'done' AND finished_at > now() - make_interval(secs => $4))) \
                 ORDER BY created_at DESC LIMIT 1",
                vec![
                    org_id.into(),
                    user.into(),
                    sql.into(),
                    f64::from(age).into(),
                ],
            ))
            .await?;
        if let Some(r) = earlier {
            return get(db, org_id, user, r.try_get("", "id")?).await;
        }
    }
    let busy = db
        .query_one_raw(stmt(
            "SELECT 1 AS x FROM analysis_queries WHERE user_id = $1 AND state IN ('queued', 'running')",
            vec![user.into()],
        ))
        .await?
        .is_some();
    if busy {
        return Err(AnalysisError::Busy);
    }
    let (tables, refused) = readable_tables(db, org_id, user, sql).await?;
    let id = Uuid::new_v4();
    let objects = freeze_objects(db, store, org_id, id, hide, sql).await?;
    let done = db
        .execute_raw(stmt(
            "INSERT INTO analysis_queries (id, org_id, user_id, sql, tables, refused, objects) \
             VALUES ($1, $2, $3, $4, $5::jsonb, $6::jsonb, $7::jsonb) ON CONFLICT DO NOTHING",
            vec![
                id.into(),
                org_id.into(),
                user.into(),
                sql.into(),
                Value::from(tables).to_string().into(),
                Value::from(refused).to_string().into(),
                Value::from(objects).to_string().into(),
            ],
        ))
        .await?;
    // Two sends at once: the unique index keeps one.
    if done.rows_affected() == 0 {
        return Err(AnalysisError::Busy);
    }
    get(db, org_id, user, id).await
}

/// Tables of the org by name, and the marked ones `user` is not cleared for. A read
/// of a marked table the query names is journaled with its purposes (D828).
async fn readable_tables(
    db: &DatabaseConnection,
    org_id: i64,
    user: i64,
    sql: &str,
) -> Result<(serde_json::Map<String, Value>, Vec<Value>), DbErr> {
    let rows = db
        .query_all_raw(stmt(
            "SELECT d.id, d.name, \
               EXISTS (SELECT 1 FROM dataset_markings dm WHERE dm.dataset_id = d.id) AS marked, \
               EXISTS (SELECT 1 FROM dataset_markings dm WHERE dm.dataset_id = d.id \
                 AND NOT EXISTS (SELECT 1 FROM clearances c WHERE c.marking_id = dm.marking_id \
                   AND c.user_id = $2 AND c.revoked_at IS NULL AND c.expires_at > now())) AS refused \
             FROM datasets d WHERE d.org_id = $1",
            vec![org_id.into(), user.into()],
        ))
        .await?;
    let named = |name: &str| {
        let lower = sql.to_ascii_lowercase();
        lower
            .match_indices(&name.to_ascii_lowercase())
            .any(|(i, _)| {
                let before = lower[..i].chars().last();
                let after = lower[i + name.len()..].chars().next();
                let edge =
                    |c: Option<char>| !c.is_some_and(|c| c.is_ascii_alphanumeric() || c == '_');
                edge(before) && edge(after)
            })
    };
    let (mut tables, mut refused) = (serde_json::Map::new(), Vec::new());
    for r in &rows {
        let id: Uuid = r.try_get("", "id")?;
        let name: String = r.try_get("", "name")?;
        if r.try_get::<bool>("", "refused")? {
            refused.push(Value::String(name));
            continue;
        }
        if r.try_get::<bool>("", "marked")? && named(&name) {
            let check =
                crate::services::governance::check_dataset_read(db, org_id, user, id, "sql", "ui");
            let _ = check.await;
        }
        tables.insert(name, Value::String(id.to_string()));
    }
    Ok((tables, refused))
}

/// One cell of an object view as text: what a table of text columns holds.
fn text_of(v: &Value) -> Option<String> {
    match v {
        Value::Null => None,
        Value::String(s) => Some(s.clone()),
        other => Some(other.to_string()),
    }
}

/// D825: each `objects.<type>` the query names, frozen as `hide` lets the author see
/// it (hidden objects absent, masked properties empty), in a Parquet file.
async fn freeze_objects(
    db: &DatabaseConnection,
    store: &S3Store,
    org_id: i64,
    id: Uuid,
    hide: Redaction,
    sql: &str,
) -> Result<serde_json::Map<String, Value>, AnalysisError> {
    use crate::services::ontology::{query, schema};
    use pnex_core::ontology::api::{OntologyQuery, QUERY_LIMIT_MAX};
    let wanted: Vec<String> = object_types(sql)
        .into_iter()
        .take(OBJECT_TYPES_MAX)
        .collect();
    let mut out = serde_json::Map::new();
    if wanted.is_empty() {
        return Ok(out);
    }
    let s = schema(db, org_id).await.map_err(|e| {
        AnalysisError::Coded(
            err_codes::ANALYSIS_UNAVAILABLE,
            json!({ "reason": e.to_string() }),
        )
    })?;
    for type_key in wanted {
        let Some(def) = s.object_type(&type_key) else {
            // Left out: the worker answers that the table does not exist.
            continue;
        };
        let props: Vec<String> = def.properties.iter().map(|p| p.key.clone()).collect();
        let mut columns: Vec<Vec<Option<String>>> = vec![Vec::new(); props.len() + 2];
        let mut offset = 0u64;
        loop {
            let q = OntologyQuery {
                type_key: Some(type_key.clone()),
                limit: Some(QUERY_LIMIT_MAX),
                offset: Some(offset),
                ..Default::default()
            };
            let mut page = query::run(db, None, org_id, &q, hide.clone())
                .await
                .map_err(|e| {
                    AnalysisError::Coded(
                        err_codes::ANALYSIS_UNAVAILABLE,
                        json!({ "reason": e.to_string() }),
                    )
                })?;
            hide.query(&s, &mut page);
            if page.count as u64 > OBJECTS_MAX {
                let args = json!({ "type": type_key, "max": OBJECTS_MAX });
                return Err(AnalysisError::Coded(
                    err_codes::ANALYSIS_TOO_MANY_OBJECTS,
                    args,
                ));
            }
            for row in &page.rows {
                let o = &row.object;
                columns[0].push(Some(o.id.clone()));
                columns[1].push(Some(o.title.clone()));
                for (i, p) in props.iter().enumerate() {
                    columns[i + 2].push(o.properties.get(p).and_then(text_of));
                }
            }
            offset += page.rows.len() as u64;
            if page.rows.is_empty() || offset >= page.count as u64 {
                break;
            }
        }
        let names: Vec<String> = ["id".to_string(), "title".to_string()]
            .into_iter()
            .chain(props)
            .collect();
        let schema = Arc::new(Schema::new(
            names
                .iter()
                .map(|n| Field::new(n, DataType::Utf8, true))
                .collect::<Vec<_>>(),
        ));
        let arrays: Vec<ArrayRef> = columns
            .into_iter()
            .map(|c| Arc::new(StringArray::from(c)) as ArrayRef)
            .collect();
        let bytes = RecordBatch::try_new(schema.clone(), arrays)
            .map_err(|e| e.to_string())
            .and_then(|batch| {
                let mut buf = Vec::new();
                let mut w = ::parquet::arrow::ArrowWriter::try_new(&mut buf, schema, None)
                    .map_err(|e| e.to_string())?;
                w.write(&batch).map_err(|e| e.to_string())?;
                w.close().map_err(|e| e.to_string())?;
                Ok(buf)
            })
            .map_err(|e| {
                AnalysisError::Coded(err_codes::ANALYSIS_UNAVAILABLE, json!({ "reason": e }))
            })?;
        let key = format!("{}{type_key}.parquet", prefix(org_id, id));
        store.put_bytes(&key, bytes.into()).await.map_err(|e| {
            AnalysisError::Coded(err_codes::ANALYSIS_UNAVAILABLE, json!({ "reason": e }))
        })?;
        out.insert(type_key, Value::String(key));
    }
    Ok(out)
}

const TS: &str = "'YYYY-MM-DD\"T\"HH24:MI:SS.MS\"Z\"'";

fn view(r: &QueryResult) -> Result<AnalysisQuery, DbErr> {
    let state: String = r.try_get("", "state")?;
    let result: Option<Value> = r.try_get("", "result")?;
    let result = result.unwrap_or(Value::Null);
    let rows: Vec<Vec<Option<String>>> =
        serde_json::from_value(result["rows"].clone()).unwrap_or_default();
    Ok(AnalysisQuery {
        id: r.try_get::<Uuid>("", "id")?.to_string(),
        state: match state.as_str() {
            "running" => AnalysisState::Running,
            "done" => AnalysisState::Done,
            "failed" => AnalysisState::Failed,
            _ => AnalysisState::Queued,
        },
        sql: r.try_get("", "sql")?,
        columns: serde_json::from_value(result["columns"].clone()).unwrap_or_default(),
        rows,
        truncated: result["truncated"].as_bool().unwrap_or(false),
        code: r.try_get("", "code")?,
        args: r
            .try_get::<Option<Value>>("", "args")?
            .unwrap_or(Value::Null),
        created_at: r.try_get("", "created_at")?,
        finished_at: r.try_get("", "finished_at")?,
    })
}

/// A query of its author only: anyone else gets not found.
pub async fn get(
    db: &DatabaseConnection,
    org_id: i64,
    user: i64,
    id: Uuid,
) -> Result<AnalysisQuery, AnalysisError> {
    let sql = format!(
        "SELECT id, state, sql, result, code, args, \
           to_char(created_at AT TIME ZONE 'UTC', {TS}) AS created_at, \
           to_char(finished_at AT TIME ZONE 'UTC', {TS}) AS finished_at \
         FROM analysis_queries WHERE org_id = $1 AND user_id = $2 AND id = $3"
    );
    let row = db
        .query_one_raw(stmt(&sql, vec![org_id.into(), user.into(), id.into()]))
        .await?
        .ok_or(AnalysisError::NotFound)?;
    Ok(view(&row)?)
}

/// Queries kept a day (D826); their frozen objects go with them.
// shortcut: a frozen object file of a query the worker never finished stays until this
// purge; delete it at the end of the run if the store grows.
pub async fn purge(db: &DatabaseConnection, store: &S3Store) -> Result<u64, DbErr> {
    let old = db
        .query_all_raw(stmt(
            "DELETE FROM analysis_queries WHERE created_at < now() - interval '1 day' \
             RETURNING org_id, objects",
            vec![],
        ))
        .await?;
    for r in &old {
        let objects: Value = r.try_get("", "objects")?;
        for key in objects
            .as_object()
            .into_iter()
            .flatten()
            .filter_map(|(_, v)| v.as_str())
        {
            let _ = store.delete_key(key).await;
        }
    }
    Ok(old.len() as u64)
}
