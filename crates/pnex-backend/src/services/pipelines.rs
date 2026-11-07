//! Pipelines (data-pipelines.md D571, D573, D574): versioned documents, the run
//! queue the worker takes from, and lineage. The server never runs user SQL.

use pnex_core::datasets::Dataset;
use pnex_core::pipelines::{
    DatasetLineage, LineageLink, Pipeline, PipelineDocument, PipelineRun, PipelineRunState,
    PipelineVersion, PipelineViolation, DOCUMENT_MAX_BYTES, NAME_MAX,
};
use sea_orm::{
    ConnectionTrait, DatabaseBackend, DatabaseConnection, DbErr, QueryResult, Statement,
    TransactionTrait,
};
use serde_json::Value;
use uuid::Uuid;

#[derive(Debug)]
pub enum PipelineError {
    Invalid {
        field: String,
        token: String,
    },
    Document(Vec<PipelineViolation>),
    NotFound,
    NameTaken,
    VersionConflict {
        current: i32,
    },
    /// A run of this pipeline is already queued or running.
    RunActive,
    RunNotFound,
    Db(DbErr),
}

impl From<DbErr> for PipelineError {
    fn from(e: DbErr) -> Self {
        Self::Db(e)
    }
}

fn stmt(sql: &str, values: Vec<sea_orm::Value>) -> Statement {
    Statement::from_sql_and_values(DatabaseBackend::Postgres, sql, values)
}

const DATE: &str = "'YYYY-MM-DD\"T\"HH24:MI:SS.MS\"Z\"'";

fn at(column: &str) -> String {
    format!("to_char({column} AT TIME ZONE 'UTC', {DATE}) AS {column}")
}

/// Parses and checks a YAML document: it can run, or the caller knows why not.
pub fn checked(yaml: &str) -> Result<PipelineDocument, PipelineError> {
    let one = |token: &str| {
        PipelineError::Document(vec![PipelineViolation {
            path: "document".into(),
            token: token.into(),
        }])
    };
    if yaml.len() > DOCUMENT_MAX_BYTES {
        return Err(one(&format!("max_bytes:{DOCUMENT_MAX_BYTES}")));
    }
    let doc: PipelineDocument = serde_saphyr::from_str(yaml).map_err(|_| one("yaml"))?;
    match doc.violations() {
        v if v.is_empty() => Ok(doc),
        v => Err(PipelineError::Document(v)),
    }
}

/// A document that closes a loop with other pipelines or imports could never be
/// ordered (D575): refused at save.
async fn no_cycle(
    db: &DatabaseConnection,
    org_id: i64,
    id: Uuid,
    name: &str,
    doc: &PipelineDocument,
) -> Result<(), PipelineError> {
    let node = crate::services::data_builds::pipeline_node(id, name.to_string(), Some(doc));
    match crate::services::data_builds::cycle_with(db, org_id, node).await? {
        Some(_) => Err(PipelineError::Document(vec![PipelineViolation {
            path: "transforms".into(),
            token: "cycle".into(),
        }])),
        None => Ok(()),
    }
}

/// Sets when the pipeline runs on its own, or stops it. `user` answers for each
/// unattended build: revalidated at every start (R2).
pub async fn schedule(
    db: &DatabaseConnection,
    org_id: i64,
    id: Uuid,
    recurrence: Option<&Value>,
    user: i64,
) -> Result<Pipeline, PipelineError> {
    get(db, org_id, id).await?;
    use crate::services::ontology::imports::next_run;
    use pnex_core::ontology::recurrence::Recurrence;
    if recurrence.is_some_and(|r| Recurrence::parse(r).is_none()) {
        return Err(PipelineError::Invalid {
            field: "recurrence".into(),
            token: "invalid".into(),
        });
    }
    db.execute_raw(stmt(
        "UPDATE pipelines SET recurrence = $3, next_run_at = $4, scheduled_by = $5 \
         WHERE org_id = $1 AND id = $2",
        vec![
            org_id.into(),
            id.into(),
            recurrence.cloned().into(),
            next_run(recurrence).into(),
            recurrence.is_some().then_some(user).into(),
        ],
    ))
    .await?;
    get(db, org_id, id).await
}

fn pipeline_select() -> String {
    format!(
        "SELECT id, name, version, document, recurrence, {}, {}, {} FROM pipelines",
        at("next_run_at"),
        at("created_at"),
        at("updated_at")
    )
}

fn pipeline_of(row: &QueryResult) -> Result<Pipeline, DbErr> {
    Ok(Pipeline {
        id: row.try_get::<Uuid>("", "id")?.to_string(),
        name: row.try_get("", "name")?,
        version: row.try_get("", "version")?,
        document: row.try_get("", "document")?,
        recurrence: row.try_get("", "recurrence")?,
        next_run_at: row.try_get("", "next_run_at")?,
        created_at: row.try_get("", "created_at")?,
        updated_at: row.try_get("", "updated_at")?,
    })
}

fn run_select() -> String {
    format!(
        "SELECT id, version, state, attempts, code, args, {}, {}, {} FROM pipeline_jobs",
        at("created_at"),
        at("started_at"),
        at("finished_at")
    )
}

fn run_of(row: &QueryResult) -> Result<PipelineRun, DbErr> {
    let state: String = row.try_get("", "state")?;
    Ok(PipelineRun {
        id: row.try_get::<Uuid>("", "id")?.to_string(),
        version: row.try_get("", "version")?,
        state: match state.as_str() {
            "queued" => PipelineRunState::Queued,
            "running" => PipelineRunState::Running,
            "done" => PipelineRunState::Done,
            _ => PipelineRunState::Failed,
        },
        attempts: row.try_get("", "attempts")?,
        code: row.try_get("", "code")?,
        args: row.try_get("", "args")?,
        created_at: row.try_get("", "created_at")?,
        started_at: row.try_get("", "started_at")?,
        finished_at: row.try_get("", "finished_at")?,
    })
}

pub async fn create(
    db: &DatabaseConnection,
    org_id: i64,
    name: &str,
    document: &str,
    author: i64,
) -> Result<Pipeline, PipelineError> {
    let name = name.trim();
    let invalid = |token: String| PipelineError::Invalid {
        field: "name".into(),
        token,
    };
    if name.is_empty() {
        return Err(invalid("required".into()));
    }
    if name.chars().count() > NAME_MAX {
        return Err(invalid(format!("max_length:{NAME_MAX}")));
    }
    let doc = checked(document)?;
    no_cycle(db, org_id, Uuid::new_v4(), name, &doc).await?;
    let tx = db.begin().await?;
    let row = tx
        .query_one_raw(stmt(
            "INSERT INTO pipelines (org_id, name, document) VALUES ($1, $2, $3) \
             ON CONFLICT (org_id, name) DO NOTHING RETURNING id",
            vec![org_id.into(), name.into(), document.into()],
        ))
        .await?
        .ok_or(PipelineError::NameTaken)?;
    let id: Uuid = row.try_get("", "id")?;
    tx.execute_raw(stmt(
        "INSERT INTO pipeline_versions (pipeline_id, version, document, author_id) \
         VALUES ($1, 1, $2, $3)",
        vec![id.into(), document.into(), author.into()],
    ))
    .await?;
    tx.commit().await?;
    get(db, org_id, id).await
}

pub async fn get(
    db: &DatabaseConnection,
    org_id: i64,
    id: Uuid,
) -> Result<Pipeline, PipelineError> {
    let sql = format!("{} WHERE org_id = $1 AND id = $2", pipeline_select());
    let row = db
        .query_one_raw(stmt(&sql, vec![org_id.into(), id.into()]))
        .await?
        .ok_or(PipelineError::NotFound)?;
    Ok(pipeline_of(&row)?)
}

async fn count(
    db: &DatabaseConnection,
    sql: &str,
    values: Vec<sea_orm::Value>,
) -> Result<i64, DbErr> {
    db.query_one_raw(stmt(sql, values))
        .await?
        .map_or(Ok(0), |r| r.try_get("", "n"))
}

pub async fn list(
    db: &DatabaseConnection,
    org_id: i64,
    limit: i64,
    offset: i64,
) -> Result<(i64, Vec<Pipeline>), PipelineError> {
    let total = count(
        db,
        "SELECT count(*) AS n FROM pipelines WHERE org_id = $1",
        vec![org_id.into()],
    )
    .await?;
    let sql = format!(
        "{} WHERE org_id = $1 ORDER BY name LIMIT $2 OFFSET $3",
        pipeline_select()
    );
    let rows = db
        .query_all_raw(stmt(&sql, vec![org_id.into(), limit.into(), offset.into()]))
        .await?;
    let items = rows.iter().map(pipeline_of).collect::<Result<_, _>>()?;
    Ok((total, items))
}

/// Saves a new version over the one that was read.
pub async fn update(
    db: &DatabaseConnection,
    org_id: i64,
    id: Uuid,
    document: &str,
    expected_version: i32,
    author: i64,
) -> Result<Pipeline, PipelineError> {
    let current = get(db, org_id, id).await?;
    let doc = checked(document)?;
    no_cycle(db, org_id, id, &current.name, &doc).await?;
    let tx = db.begin().await?;
    let done = tx
        .execute_raw(stmt(
            // A schedule runs on the rights of who last wrote the document (SEC-58).
            "UPDATE pipelines SET document = $3, version = version + 1, updated_at = now(), \
             scheduled_by = CASE WHEN scheduled_by IS NULL THEN NULL ELSE $5 END \
             WHERE org_id = $1 AND id = $2 AND version = $4",
            vec![
                org_id.into(),
                id.into(),
                document.into(),
                expected_version.into(),
                author.into(),
            ],
        ))
        .await?;
    if done.rows_affected() == 0 {
        return Err(PipelineError::VersionConflict {
            current: current.version,
        });
    }
    tx.execute_raw(stmt(
        "INSERT INTO pipeline_versions (pipeline_id, version, document, author_id) \
         VALUES ($1, $2, $3, $4)",
        vec![
            id.into(),
            (expected_version + 1).into(),
            document.into(),
            author.into(),
        ],
    ))
    .await?;
    tx.commit().await?;
    get(db, org_id, id).await
}

pub async fn delete(db: &DatabaseConnection, org_id: i64, id: Uuid) -> Result<(), PipelineError> {
    // Lock the pipeline first, then count active jobs in a new statement: its
    // fresh snapshot sees a run committed while we waited for the lock, and a
    // run started after the lock waits on the FK and fails once we are gone.
    // One statement with NOT EXISTS would miss the first case under READ
    // COMMITTED, and the cascade would delete a job under the worker.
    let tx = db.begin().await?;
    tx.query_one_raw(stmt(
        "SELECT id FROM pipelines WHERE org_id = $1 AND id = $2 FOR UPDATE",
        vec![org_id.into(), id.into()],
    ))
    .await?
    .ok_or(PipelineError::NotFound)?;
    let active: i64 = tx
        .query_one_raw(stmt(
            "SELECT count(*) AS n FROM pipeline_jobs WHERE pipeline_id = $1 \
             AND state IN ('queued', 'running')",
            vec![id.into()],
        ))
        .await?
        .map_or(Ok(0), |r| r.try_get("", "n"))?;
    if active > 0 {
        return Err(PipelineError::RunActive);
    }
    tx.execute_raw(stmt(
        "DELETE FROM pipelines WHERE org_id = $1 AND id = $2",
        vec![org_id.into(), id.into()],
    ))
    .await?;
    tx.commit().await?;
    Ok(())
}

pub async fn versions(
    db: &DatabaseConnection,
    org_id: i64,
    id: Uuid,
    limit: i64,
    offset: i64,
) -> Result<(i64, Vec<PipelineVersion>), PipelineError> {
    get(db, org_id, id).await?;
    let total = count(
        db,
        "SELECT count(*) AS n FROM pipeline_versions WHERE pipeline_id = $1",
        vec![id.into()],
    )
    .await?;
    let sql = format!(
        "SELECT v.version, v.document, u.email AS author, {} FROM pipeline_versions v \
         LEFT JOIN organization_members m ON m.user_id = v.author_id AND m.org_id = $4 \
         LEFT JOIN users u ON u.id = m.user_id WHERE v.pipeline_id = $1 \
         ORDER BY v.version DESC LIMIT $2 OFFSET $3",
        "to_char(v.created_at AT TIME ZONE 'UTC', 'YYYY-MM-DD\"T\"HH24:MI:SS\"Z\"') AS created_at"
    );
    let rows = db
        .query_all_raw(stmt(
            &sql,
            vec![id.into(), limit.into(), offset.into(), org_id.into()],
        ))
        .await?;
    let mut items = Vec::with_capacity(rows.len());
    for r in &rows {
        items.push(PipelineVersion {
            version: r.try_get("", "version")?,
            document: r.try_get("", "document")?,
            author: r.try_get("", "author")?,
            created_at: r.try_get("", "created_at")?,
        });
    }
    Ok((total, items))
}

/// Queues a run of the current version.
pub async fn start_run(
    db: &DatabaseConnection,
    org_id: i64,
    id: Uuid,
) -> Result<PipelineRun, PipelineError> {
    let pipeline = get(db, org_id, id).await?;
    let row = db
        .query_one_raw(stmt(
            "INSERT INTO pipeline_jobs (org_id, pipeline_id, version, trigger) \
             VALUES ($1, $2, $3, 'manual') \
             ON CONFLICT (pipeline_id) WHERE state IN ('queued', 'running') DO NOTHING \
             RETURNING id",
            vec![org_id.into(), id.into(), pipeline.version.into()],
        ))
        .await?
        .ok_or(PipelineError::RunActive)?;
    get_run(db, org_id, id, row.try_get("", "id")?).await
}

pub async fn get_run(
    db: &DatabaseConnection,
    org_id: i64,
    id: Uuid,
    run_id: Uuid,
) -> Result<PipelineRun, PipelineError> {
    let sql = format!(
        "{} WHERE org_id = $1 AND pipeline_id = $2 AND id = $3",
        run_select()
    );
    let row = db
        .query_one_raw(stmt(&sql, vec![org_id.into(), id.into(), run_id.into()]))
        .await?
        .ok_or(PipelineError::RunNotFound)?;
    Ok(run_of(&row)?)
}

pub async fn runs(
    db: &DatabaseConnection,
    org_id: i64,
    id: Uuid,
    limit: i64,
    offset: i64,
) -> Result<(i64, Vec<PipelineRun>), PipelineError> {
    get(db, org_id, id).await?;
    let total = count(
        db,
        "SELECT count(*) AS n FROM pipeline_jobs WHERE pipeline_id = $1",
        vec![id.into()],
    )
    .await?;
    let sql = format!(
        "{} WHERE pipeline_id = $1 ORDER BY pipeline_jobs.created_at DESC LIMIT $2 OFFSET $3",
        run_select()
    );
    let rows = db
        .query_all_raw(stmt(&sql, vec![id.into(), limit.into(), offset.into()]))
        .await?;
    let items = rows.iter().map(run_of).collect::<Result<_, _>>()?;
    Ok((total, items))
}

fn link_of(row: &QueryResult) -> Result<LineageLink, DbErr> {
    Ok(LineageLink {
        dataset_id: row.try_get::<Uuid>("", "dataset_id")?.to_string(),
        dataset_name: row.try_get("", "dataset_name")?,
        pipeline_id: row.try_get::<Uuid>("", "pipeline_id")?.to_string(),
        pipeline_name: row.try_get("", "pipeline_name")?,
    })
}

/// Lineage of a dataset, from what its runs read (D574). `dataset` was read
/// under the caller's org: both ends of a link belong to it by construction,
/// the worker resolves names inside the org of the job.
pub async fn lineage(db: &DatabaseConnection, dataset: &Dataset) -> Result<DatasetLineage, DbErr> {
    let id: Uuid = dataset.id.parse().map_err(|_| DbErr::Custom("id".into()))?;
    let upstream = db
        .query_all_raw(stmt(
            "SELECT DISTINCT d.id AS dataset_id, d.name AS dataset_name, \
             p.id AS pipeline_id, p.name AS pipeline_name \
             FROM dataset_transactions t \
             JOIN dataset_transaction_inputs i ON i.transaction_id = t.id \
             JOIN dataset_transactions it ON it.id = i.input_transaction_id \
             JOIN datasets d ON d.id = it.dataset_id \
             JOIN pipelines p ON p.id = i.pipeline_id \
             WHERE t.dataset_id = $1 ORDER BY dataset_name, pipeline_name",
            vec![id.into()],
        ))
        .await?;
    let downstream = db
        .query_all_raw(stmt(
            "SELECT DISTINCT d.id AS dataset_id, d.name AS dataset_name, \
             p.id AS pipeline_id, p.name AS pipeline_name \
             FROM dataset_transactions it \
             JOIN dataset_transaction_inputs i ON i.input_transaction_id = it.id \
             JOIN dataset_transactions t ON t.id = i.transaction_id \
             JOIN datasets d ON d.id = t.dataset_id \
             JOIN pipelines p ON p.id = i.pipeline_id \
             WHERE it.dataset_id = $1 ORDER BY dataset_name, pipeline_name",
            vec![id.into()],
        ))
        .await?;
    // Stale: an input of the latest computed transaction has since committed
    // a newer one.
    let stale = db
        .query_one_raw(stmt(
            "WITH latest AS ( \
               SELECT t.id FROM dataset_transactions t \
               WHERE t.dataset_id = $1 AND t.state = 'committed' \
               ORDER BY t.seq DESC LIMIT 1 \
             ) \
             SELECT EXISTS ( \
               SELECT 1 FROM dataset_transaction_inputs i \
               JOIN latest ON latest.id = i.transaction_id \
               JOIN dataset_transactions it ON it.id = i.input_transaction_id \
               JOIN dataset_transactions newer ON newer.dataset_id = it.dataset_id \
                 AND newer.state = 'committed' AND newer.seq > ( \
                   SELECT max(r.seq) FROM dataset_transaction_inputs ri \
                   JOIN dataset_transactions r ON r.id = ri.input_transaction_id \
                   WHERE ri.transaction_id = latest.id AND r.dataset_id = it.dataset_id) \
             ) AS stale",
            vec![id.into()],
        ))
        .await?
        .map_or(Ok(false), |r| r.try_get("", "stale"))?;
    Ok(DatasetLineage {
        upstream: upstream.iter().map(link_of).collect::<Result<_, _>>()?,
        downstream: downstream.iter().map(link_of).collect::<Result<_, _>>()?,
        stale,
    })
}

#[cfg(test)]
mod yaml_tests {
    use super::*;
    use pnex_core::pipelines::{Checks, Transform};
    use std::collections::BTreeMap;

    fn doc(sql: &str) -> PipelineDocument {
        PipelineDocument {
            transforms: vec![Transform {
                output: "clean".into(),
                inputs: BTreeMap::from([("a".to_string(), "raw: \"x\"".to_string())]),
                sql: sql.into(),
                checks: Checks {
                    min_rows: Some(1),
                    unique: vec!["id".into()],
                    max_empty_pct: BTreeMap::from([("name".to_string(), 5)]),
                    same_columns: true,
                    ..Checks::default()
                },
                retain_days: None,
            }],
        }
    }

    /// What a screen writes is what the server reads, whatever the query holds.
    #[test]
    fn a_document_survives_its_yaml() {
        for sql in [
            "SELECT 1",
            "SELECT *\nFROM a\n\nWHERE x = 'y: z' -- comment # hash",
            "SELECT '{not: yaml}', \"é\", '|', '- x'",
            "  SELECT 1",
            "SELECT 1\n",
            "\nSELECT 1",
            "SELECT 1 \nFROM a",
            "SELECT\t1",
            "SELECT 1\r\nFROM a",
            "SELECT 1\n  , 2\n\n\n  , 3",
            "SELECT '\\n' -- \u{1F600}",
        ] {
            let d = doc(sql);
            let parsed: PipelineDocument =
                serde_saphyr::from_str(&d.to_yaml()).unwrap_or_else(|e| panic!("{sql:?}: {e}"));
            assert_eq!(parsed, d, "{sql:?}\n{}", d.to_yaml());
        }
    }

    #[test]
    fn a_plain_query_stays_readable() {
        let yaml = doc("SELECT *\nFROM a").to_yaml();
        assert!(
            yaml.contains("sql: |-\n      SELECT *\n      FROM a\n"),
            "{yaml}"
        );
    }

    #[test]
    fn the_starter_can_run() {
        let d = PipelineDocument::starter("raw", "clean");
        assert_eq!(checked(&d.to_yaml()).unwrap(), d);
    }
}
