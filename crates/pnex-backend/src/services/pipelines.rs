//! An org's own pipelines (pipelines.md D934, D571, D573, D574): versioned
//! engine documents, checked and tested before they are saved, their run
//! queue, and lineage. The server runs their steps; the worker runs their
//! SQL, never the server.

use std::collections::HashMap;
use std::time::Duration;

use loco_rs::app::AppContext;
use pnex_core::datasets::Dataset;
use pnex_core::pipeline::doc::Document;
use pnex_core::pipelines::{
    DatasetLineage, LineageLink, Pipeline, PipelineCheck, PipelineRun, PipelineRunState,
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
    /// The caller is not cleared for these markings of the tables it reads (D801).
    MarkingRequired(Vec<String>),
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

fn violation(path: String, token: String) -> PipelineViolation {
    PipelineViolation { path, token }
}

/// Parses and checks a YAML document: it can run, or the caller knows why not.
pub fn checked(yaml: &str) -> Result<Document, PipelineError> {
    let one =
        |token: &str| PipelineError::Document(vec![violation("document".into(), token.into())]);
    if yaml.len() > DOCUMENT_MAX_BYTES {
        return Err(one(&format!("max_bytes:{DOCUMENT_MAX_BYTES}")));
    }
    let doc: Document = serde_saphyr::from_str(yaml).map_err(|_| one("yaml"))?;
    let v: Vec<PipelineViolation> = doc
        .violations()
        .into_iter()
        .chain(doc.org_violations())
        .map(|v| violation(v.path, v.token))
        .collect();
    match v.is_empty() {
        true => Ok(doc),
        false => Err(PipelineError::Document(v)),
    }
}

/// How long the tests of one save may take, the worker included.
const TESTS_WAIT: Duration = Duration::from_secs(60);

/// Plays the document's `tests:` on the answers they hold (D934): a case
/// that does not hold refuses the document.
pub async fn tested(ctx: &AppContext, org_id: i64, doc: &Document) -> Result<(), PipelineError> {
    use crate::services::pipeline_engine::{replay, sql::WorkerSql, store};
    if doc.tests.is_empty() {
        return Ok(());
    }
    let storage = crate::services::datasets::store_of(&ctx.config)
        .map_err(|e| PipelineError::Db(DbErr::Custom(e.code().to_string())))?;
    let sql = WorkerSql {
        db: &ctx.db,
        store: &storage,
        org: org_id,
    };
    let own = replay::Replay {
        pack: None,
        given: HashMap::new(),
    };
    let functions = store::own_functions(&ctx.db, org_id, doc).await?;
    let all = async {
        let mut out = Vec::new();
        for (i, case) in doc.tests.iter().enumerate() {
            let functions = functions.clone();
            let extractors = store::model_extractors("", doc);
            for p in replay::case(doc, i, case, &own, functions, extractors, &sql).await {
                let path = match p.table {
                    Some(t) => format!("tests.{i}.expect.{t}"),
                    None => format!("tests.{i}"),
                };
                out.push(violation(path, p.token));
            }
        }
        out
    };
    let v = tokio::time::timeout(TESTS_WAIT, all)
        .await
        .unwrap_or_else(|_| vec![violation("tests".into(), "timeout".into())]);
    match v.is_empty() {
        true => Ok(()),
        false => Err(PipelineError::Document(v)),
    }
}

/// The document of a probe of the step that outputs `step` (D944): cut
/// after it, its writes left out, one row read. A navigation is refused: it
/// is a mission of the member's browser agent, not a read.
pub fn probed(yaml: &str, step: &str) -> Result<Document, PipelineError> {
    use pnex_core::pipeline::doc::Step;
    let mut doc = checked(yaml)?;
    let invalid = |token: &str| PipelineError::Invalid {
        field: "step".into(),
        token: token.into(),
    };
    let k = doc
        .steps
        .iter()
        .position(|s| s.output() == Some(step))
        .ok_or_else(|| invalid("unknown"))?;
    doc.steps.truncate(k + 1);
    if doc.steps.iter().any(|s| matches!(s, Step::Browse(_))) {
        return Err(invalid("browse"));
    }
    doc.steps.retain(|s| !matches!(s, Step::Write(_)));
    // A tracked first step takes at most the rows a run is given: one.
    if let Some(Step::Enumerate(e)) = doc.steps.first_mut() {
        e.track = true;
    }
    Ok(doc)
}

/// What a preview of objects compares to (D1050): the saved pipeline whose
/// base it reads, and whether the caller may read the ontology.
pub struct Preview<'a> {
    /// The whole document, its `write: objects` steps included.
    pub full: &'a Document,
    pub pipeline: Option<Uuid>,
    pub may_read: bool,
}

/// Plays a probe's document (from [`probed`]) under the caller's markings:
/// its steps and the first rows of its last one. Nothing is kept. A
/// `write: objects` step reading the probed step counts what it would write.
pub async fn probe(
    ctx: &AppContext,
    org_id: i64,
    user: i64,
    document: &str,
    doc: &Document,
    asked: Vec<serde_json::Map<String, Value>>,
    preview: Preview<'_>,
) -> Result<pnex_core::pipelines::PipelineProbeResult, PipelineError> {
    use crate::services::pipeline_engine::{sample, store};
    use pnex_core::pipelines::{
        PipelineProbeResult, PipelineProbeStep, PROBE_FILE_BYTES, PROBE_ROWS_MAX,
    };
    let missing = missing_markings(&ctx.db, org_id, user, document).await?;
    if !missing.is_empty() {
        return Err(PipelineError::MarkingRequired(missing));
    }
    let storage = |code: &str| PipelineError::Db(DbErr::Custom(code.to_string()));
    let (journal, last) =
        store::probe_own(ctx, org_id, doc, Some(user), asked, &store::HttpTransport)
            .await
            .map_err(|e| storage(e.code()))?;
    let probed = doc.steps.last().and_then(|s| s.output());
    let target = preview.full.steps.iter().find_map(|s| match s {
        pnex_core::pipeline::doc::Step::Write(w) if Some(w.from.as_str()) == probed => {
            w.objects.as_ref().map(|o| (w.from.as_str(), o))
        }
        _ => None,
    });
    let objects = match (&last, target, preview.may_read) {
        (Some(t), Some((from, target)), true) => {
            let reads = preview.full.table_reads();
            objects_preview(ctx, org_id, user, t, from, target, &reads, preview.pipeline).await?
        }
        _ => None,
    };
    let sample = match last {
        Some(t) => sample(&t, PROBE_ROWS_MAX, PROBE_FILE_BYTES).map_err(|f| storage(f.code))?,
        None => Vec::new(),
    };
    let steps = journal
        .into_iter()
        .map(|r| PipelineProbeStep {
            step: r.step.to_string(),
            rows: r.rows,
            bytes: r.bytes,
            url: r.url,
            code: r.failure.as_ref().map(|f| f.code.to_string()),
            args: r.failure.map(|f| f.args).filter(|a| !a.is_null()),
        })
        .collect();
    Ok(PipelineProbeResult {
        steps,
        sample,
        objects,
    })
}

/// What `target` would write from the rows of `table`, against the base of
/// `pipeline` when given: `None` when the step writes nothing the org has.
#[allow(clippy::too_many_arguments)]
async fn objects_preview(
    ctx: &AppContext,
    org_id: i64,
    user: i64,
    table: &crate::services::pipeline_engine::Table,
    from: &str,
    target: &pnex_core::pipeline::doc::ObjectsTarget,
    reads: &[&str],
    pipeline: Option<Uuid>,
) -> Result<Option<pnex_core::pipelines::ObjectsPreview>, PipelineError> {
    use crate::services::ontology::{imports, OntologyError};
    let failed = |e: String| PipelineError::Db(DbErr::Custom(e));
    let rows = crate::services::pipeline_engine::records(table)
        .await
        .map_err(|f| failed(f.code.to_string()))?;
    let schema = crate::services::ontology::schema(&ctx.db, org_id).await?;
    let key = pipeline.map(|p| p.to_string()).unwrap_or_default();
    let Some(mapping) =
        crate::services::pipeline_engine::store::objects_mapping(&key, target, &schema)
    else {
        return Ok(None);
    };
    let source = format!("pipeline:{key}");
    let w = imports::RowsWriter {
        org_id,
        pipeline: &key,
        step: from,
        source_ref: &source,
        author: Some(user),
        reads,
        snapshot: target.snapshot,
        dry: true,
    };
    let rows: Vec<Value> = rows.into_iter().map(Value::Object).collect();
    match imports::write_rows(&ctx.db, &w, &mapping, &rows).await {
        Ok(out) => Ok(Some(pnex_core::pipelines::ObjectsPreview {
            counts: out.counts,
            warnings: out.warnings,
            errors: out.errors,
        })),
        Err(OntologyError::Db(e)) => Err(PipelineError::Db(e)),
        // A refusal (the caller writes nothing, a mapping too large): no preview.
        Err(_) => Ok(None),
    }
}

/// What a document reads and writes once checked and tested: the answer
/// of the check route.
pub async fn check(
    ctx: &AppContext,
    org_id: i64,
    yaml: &str,
) -> Result<PipelineCheck, PipelineError> {
    let doc = checked(yaml)?;
    tested(ctx, org_id, &doc).await?;
    Ok(PipelineCheck {
        reads: doc.table_reads().into_iter().map(str::to_string).collect(),
        writes: doc.table_writes().into_iter().map(str::to_string).collect(),
        steps: doc.steps.iter().map(|s| s.name().to_string()).collect(),
        probes: doc
            .steps
            .iter()
            .take_while(|s| !matches!(s, pnex_core::pipeline::doc::Step::Browse(_)))
            .filter_map(|s| s.output().map(str::to_string))
            .collect(),
        tests: doc.tests.len(),
        rights: doc.rights().into_iter().map(str::to_string).collect(),
        rule: pnex_core::site_recipe::rule_of(&doc),
    })
}

/// A document that closes a loop with other pipelines or imports could never be
/// ordered (D575): refused at save.
async fn no_cycle(
    db: &DatabaseConnection,
    org_id: i64,
    id: Uuid,
    name: &str,
    doc: &Document,
) -> Result<(), PipelineError> {
    let node = crate::services::data_builds::pipeline_node(id, name.to_string(), Some(doc));
    match crate::services::data_builds::cycle_with(db, org_id, node).await? {
        Some(_) => Err(PipelineError::Document(vec![violation(
            "steps".into(),
            "cycle".into(),
        )])),
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
    let document: String = row.try_get("", "document")?;
    Ok(Pipeline {
        id: row.try_get::<Uuid>("", "id")?.to_string(),
        name: row.try_get("", "name")?,
        version: row.try_get("", "version")?,
        rule: serde_saphyr::from_str::<Document>(&document)
            .ok()
            .as_ref()
            .and_then(pnex_core::site_recipe::rule_of),
        document,
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
        steps: Vec::new(),
    })
}

pub async fn create(
    ctx: &AppContext,
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
    let db = &ctx.db;
    no_cycle(db, org_id, Uuid::new_v4(), name, &doc).await?;
    tested(ctx, org_id, &doc).await?;
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
    declare_identity(&tx, org_id, id, &doc).await?;
    tx.commit().await?;
    get(db, org_id, id).await
}

/// The Tables a `write: objects` step reads through a `sql` step learn
/// from its mapping what erasure reaches (§3.11, D1050), as an import's
/// mapping declared on its Table.
async fn declare_identity<C: ConnectionTrait>(
    db: &C,
    org_id: i64,
    id: Uuid,
    doc: &Document,
) -> Result<(), PipelineError> {
    use pnex_core::pipeline::doc::Step;
    let failed = |e: crate::services::ontology::OntologyError| {
        PipelineError::Db(DbErr::Custom(e.to_string()))
    };
    let schema = crate::services::ontology::schema(db, org_id).await?;
    let key = id.to_string();
    for (from, target) in doc.steps.iter().filter_map(|s| match s {
        Step::Write(w) => w.objects.as_ref().map(|o| (w.from.as_str(), o)),
        _ => None,
    }) {
        // shortcut: the columns are not traced through the query; each Table
        // it reads declares those of the mapping it has.
        let Some(Step::Sql(q)) = doc.steps.iter().find(|s| s.output() == Some(from)) else {
            continue;
        };
        let Some(m) =
            crate::services::pipeline_engine::store::objects_mapping(&key, target, &schema)
        else {
            continue;
        };
        for table in q.tables.values() {
            crate::services::ontology::imports::declare_identity(db, org_id, table, &m)
                .await
                .map_err(failed)?;
        }
    }
    Ok(())
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
    ctx: &AppContext,
    org_id: i64,
    id: Uuid,
    document: &str,
    expected_version: i32,
    author: i64,
) -> Result<Pipeline, PipelineError> {
    let db = &ctx.db;
    let current = get(db, org_id, id).await?;
    let doc = checked(document)?;
    no_cycle(db, org_id, id, &current.name, &doc).await?;
    tested(ctx, org_id, &doc).await?;
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
    declare_identity(&tx, org_id, id, &doc).await?;
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

/// Queues a run of the current version; `rows` go to its `asked` axis.
pub async fn start_run(
    db: &DatabaseConnection,
    org_id: i64,
    id: Uuid,
    user: i64,
    rows: Vec<serde_json::Map<String, serde_json::Value>>,
) -> Result<PipelineRun, PipelineError> {
    let pipeline = get(db, org_id, id).await?;
    let missing = missing_markings(db, org_id, user, &pipeline.document).await?;
    if !missing.is_empty() {
        return Err(PipelineError::MarkingRequired(missing));
    }
    let row = db
        .query_one_raw(stmt(
            "INSERT INTO pipeline_jobs (org_id, pipeline_id, version, trigger, started_by, asked) \
             VALUES ($1, $2, $3, 'manual', $4, $5) \
             ON CONFLICT (pipeline_id) WHERE state IN ('queued', 'running') DO NOTHING \
             RETURNING id",
            vec![
                org_id.into(),
                id.into(),
                pipeline.version.into(),
                user.into(),
                (!rows.is_empty())
                    .then(|| serde_json::Value::from(rows))
                    .into(),
            ],
        ))
        .await?
        .ok_or(PipelineError::RunActive)?;
    get_run(db, org_id, id, row.try_get("", "id")?).await
}

/// Markings of the tables a pipeline document reads that `user` is not
/// cleared for: a run reads under its starter's rights (governance.md D801).
pub async fn missing_markings(
    db: &DatabaseConnection,
    org_id: i64,
    user: i64,
    document: &str,
) -> Result<Vec<String>, DbErr> {
    let Ok(doc) = serde_saphyr::from_str::<Document>(document) else {
        return Ok(Vec::new());
    };
    let mut out = std::collections::BTreeSet::new();
    for name in doc.table_reads() {
        let row = db
            .query_one_raw(stmt(
                "SELECT id FROM datasets WHERE org_id = $1 AND name = $2",
                vec![org_id.into(), name.into()],
            ))
            .await?;
        if let Some(id) = row.and_then(|r| r.try_get::<Uuid>("", "id").ok()) {
            out.extend(crate::services::governance::missing_for(db, org_id, user, id).await?);
        }
    }
    Ok(out.into_iter().collect())
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
    let mut run = run_of(&row)?;
    let node = format!("pipeline:{id}");
    run.steps = crate::services::ingest::run_steps(db, org_id, &node, run_id).await?;
    Ok(run)
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
        pipeline_id: row
            .try_get::<Option<Uuid>>("", "pipeline_id")?
            .map(|u| u.to_string()),
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
             p.id AS pipeline_id, coalesce(p.name, i.pipeline_key) AS pipeline_name \
             FROM dataset_transactions t \
             JOIN dataset_transaction_inputs i ON i.transaction_id = t.id \
             JOIN dataset_transactions it ON it.id = i.input_transaction_id \
             JOIN datasets d ON d.id = it.dataset_id \
             LEFT JOIN pipelines p ON p.id = i.pipeline_id \
             WHERE t.dataset_id = $1 ORDER BY dataset_name, pipeline_name",
            vec![id.into()],
        ))
        .await?;
    let downstream = db
        .query_all_raw(stmt(
            "SELECT DISTINCT d.id AS dataset_id, d.name AS dataset_name, \
             p.id AS pipeline_id, coalesce(p.name, i.pipeline_key) AS pipeline_name \
             FROM dataset_transactions it \
             JOIN dataset_transaction_inputs i ON i.input_transaction_id = it.id \
             JOIN dataset_transactions t ON t.id = i.transaction_id \
             JOIN datasets d ON d.id = t.dataset_id \
             LEFT JOIN pipelines p ON p.id = i.pipeline_id \
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

// ─────────────────────────── Runs ───────────────────────────

const HEARTBEAT_EVERY: Duration = Duration::from_secs(30);
/// Takes of one run before it fails for good.
const MAX_ATTEMPTS: i32 = 2;

/// A queued run, taken.
struct Job {
    id: Uuid,
    org_id: i64,
    pipeline_id: Uuid,
    version: i32,
    attempts: i32,
    started_by: Option<i64>,
    asked: Vec<serde_json::Map<String, serde_json::Value>>,
}

/// Runs dead for too long go back to the queue, or fail after their attempts.
async fn sweep_dead(db: &DatabaseConnection) -> Result<(), DbErr> {
    // One minute past the 10 after which a dead run's open table transaction
    // is freed: requeued sooner, the retry would find it still open.
    db.execute_raw(stmt(
        "UPDATE pipeline_jobs SET state = 'failed', code = $1, finished_at = now() \
         WHERE state = 'running' AND heartbeat_at < now() - interval '11 minutes' \
         AND attempts >= $2",
        vec![
            pnex_core::err_codes::PIPELINE_WORKER_LOST.into(),
            MAX_ATTEMPTS.into(),
        ],
    ))
    .await?;
    db.execute_raw(stmt(
        "UPDATE pipeline_jobs SET state = 'queued' \
         WHERE state = 'running' AND heartbeat_at < now() - interval '11 minutes' \
         AND attempts < $1",
        vec![MAX_ATTEMPTS.into()],
    ))
    .await?;
    Ok(())
}

/// Takes a queued run of an org with no run in progress and no erasure
/// pending (a run started now could read rows about to be erased). The org
/// served least recently goes first.
async fn claim(db: &DatabaseConnection) -> Result<Option<Job>, DbErr> {
    let row = db
        .query_one_raw(stmt(
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
             ) RETURNING id, org_id, pipeline_id, version, attempts, started_by, asked",
            vec![],
        ))
        .await;
    let row = match row {
        Ok(r) => r,
        // Two servers took two runs of one org: the unique index stopped the
        // second, its run stays queued.
        Err(e) if e.to_string().contains("duplicate key") => return Ok(None),
        Err(e) => return Err(e),
    };
    row.map(|r| {
        Ok(Job {
            id: r.try_get("", "id")?,
            org_id: r.try_get("", "org_id")?,
            pipeline_id: r.try_get("", "pipeline_id")?,
            version: r.try_get("", "version")?,
            attempts: r.try_get("", "attempts")?,
            started_by: r.try_get("", "started_by")?,
            asked: r
                .try_get::<Option<serde_json::Value>>("", "asked")?
                .and_then(|v| serde_json::from_value(v).ok())
                .unwrap_or_default(),
        })
    })
    .transpose()
}

/// What the member who started a run no longer holds of its document's
/// rights and markings (R2); `None` when nothing is missing or nobody
/// started it.
async fn denied(
    db: &DatabaseConnection,
    org: i64,
    user: Option<i64>,
    doc: &Document,
    document: &str,
) -> Result<Option<(&'static str, serde_json::Value)>, DbErr> {
    use pnex_core::err_codes;
    let Some(user) = user else {
        return Ok(None);
    };
    let grants = crate::services::iam::store::member_grants(db, org, user).await?;
    if let Some(p) = doc
        .rights()
        .into_iter()
        .find(|p| !grants.as_ref().is_some_and(|g| g.can(p)))
    {
        return Ok(Some((
            err_codes::ACCESS_DENIED,
            serde_json::json!({ "permission": p }),
        )));
    }
    let missing = missing_markings(db, org, user, document).await?;
    Ok((!missing.is_empty()).then(|| {
        (
            err_codes::MARKING_REQUIRED,
            serde_json::json!({ "markings": missing }),
        )
    }))
}

/// The current version of an org's own pipeline, for a row its navigation
/// resumes (D995). `Err` inside: the starter lost a right or a marking since.
pub async fn resumable(
    db: &DatabaseConnection,
    org: i64,
    id: Uuid,
    user: Option<i64>,
) -> Result<Option<Result<(Document, i32), (&'static str, serde_json::Value)>>, DbErr> {
    let Some(row) = db
        .query_one_raw(stmt(
            "SELECT document, version FROM pipelines WHERE id = $1 AND org_id = $2",
            vec![id.into(), org.into()],
        ))
        .await?
    else {
        return Ok(None);
    };
    let document: String = row.try_get("", "document")?;
    let version: i32 = row.try_get("", "version")?;
    let Ok(doc) = checked(&document) else {
        return Ok(None);
    };
    Ok(Some(match denied(db, org, user, &doc, &document).await? {
        Some(no) => Err(no),
        None => Ok((doc, version)),
    }))
}

/// Runs one job: the version it names, under the rights its starter still
/// holds (R2). `Err`: its failure code and arguments.
async fn run_job(ctx: &AppContext, job: &Job) -> Result<(), (String, serde_json::Value)> {
    use pnex_core::err_codes;
    let db_failure = |e: DbErr| {
        tracing::warn!(error = %e, "pipeline run: database");
        (
            err_codes::PIPELINE_WORKER_LOST.to_string(),
            serde_json::Value::Null,
        )
    };
    let document: String = ctx
        .db
        .query_one_raw(stmt(
            "SELECT document FROM pipeline_versions WHERE pipeline_id = $1 AND version = $2",
            vec![job.pipeline_id.into(), job.version.into()],
        ))
        .await
        .map_err(db_failure)?
        .and_then(|r| r.try_get("", "document").ok())
        .ok_or_else(|| {
            (
                err_codes::PIPELINE_NOT_FOUND.to_string(),
                serde_json::Value::Null,
            )
        })?;
    let doc = checked(&document).map_err(|_| {
        (
            err_codes::PIPELINE_DOCUMENT_INVALID.to_string(),
            serde_json::Value::Null,
        )
    })?;
    if let Some((code, args)) = denied(&ctx.db, job.org_id, job.started_by, &doc, &document)
        .await
        .map_err(db_failure)?
    {
        return Err((code.to_string(), args));
    }
    use crate::services::pipeline_engine::store;
    let mut steps = Vec::new();
    let out = store::run_own(
        ctx,
        job.org_id,
        job.pipeline_id,
        job.version,
        &doc,
        job.started_by,
        job.asked.clone(),
        &store::HttpTransport,
        Some(&mut steps),
    )
    .await
    .map_err(|e| (e.code().to_string(), serde_json::Value::Null))?;
    let node = format!("pipeline:{}", job.pipeline_id);
    store::journal(&ctx.db, job.org_id, &node, job.id, &steps)
        .await
        .map_err(db_failure)?;
    // D802: objects written from the org's tables carry their markings.
    let writes_objects = doc
        .steps
        .iter()
        .any(|s| matches!(s, pnex_core::pipeline::doc::Step::Write(w) if w.objects.is_some()));
    if writes_objects {
        let source = crate::services::ontology::imports::diff::source_exact(&node);
        for table in doc.table_reads() {
            let dataset = ctx
                .db
                .query_one_raw(stmt(
                    "SELECT id FROM datasets WHERE org_id = $1 AND name = $2",
                    vec![job.org_id.into(), table.into()],
                ))
                .await
                .map_err(db_failure)?
                .and_then(|r| r.try_get::<Uuid>("", "id").ok());
            if let Some(dataset) = dataset {
                crate::services::governance::propagate_sync(&ctx.db, job.org_id, &source, dataset)
                    .await
                    .map_err(db_failure)?;
            }
        }
    }
    // What the member can fix gets its message (D1050: as an import's run did).
    match out.failure {
        Some(f) => Err((
            match f.code {
                "secret_required" => err_codes::IMPORT_KEY_REQUIRED,
                "contact_required" => err_codes::IMPORT_CONTACT_REQUIRED,
                "schema_changed" => err_codes::IMPORT_SCHEMA_CHANGED,
                "entry_missing" => err_codes::INGEST_ENTRY_MISSING,
                code => code,
            }
            .to_string(),
            f.args,
        )),
        None => Ok(()),
    }
}

/// Takes and runs the next queued run. `false` when there is none.
pub async fn run_next(ctx: &AppContext) -> Result<bool, DbErr> {
    sweep_dead(&ctx.db).await?;
    let Some(job) = claim(&ctx.db).await? else {
        return Ok(false);
    };
    let beat = {
        let (db, id) = (ctx.db.clone(), job.id);
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(HEARTBEAT_EVERY).await;
                let _ = db
                    .execute_raw(stmt(
                        "UPDATE pipeline_jobs SET heartbeat_at = now() WHERE id = $1",
                        vec![id.into()],
                    ))
                    .await;
            }
        })
    };
    let outcome = run_job(ctx, &job).await;
    beat.abort();
    let (state, code, args) = match outcome {
        Ok(()) => ("done", None, None),
        Err((code, args)) => {
            tracing::info!(job = %job.id, code, "pipeline run failed");
            ("failed", Some(code), Some(args))
        }
    };
    ctx.db
        .execute_raw(stmt(
            // A late server must not overwrite a run another one took back.
            "UPDATE pipeline_jobs SET state = $2, code = $3, args = $4, finished_at = now() \
             WHERE id = $1 AND state = 'running' AND attempts = $5",
            vec![
                job.id.into(),
                state.into(),
                code.into(),
                args.into(),
                job.attempts.into(),
            ],
        ))
        .await?;
    Ok(true)
}

/// The queue of the org pipelines' runs, polled every 2 seconds.
pub fn spawn(ctx: &AppContext) {
    let ctx = ctx.clone();
    tokio::spawn(async move {
        loop {
            match run_next(&ctx).await {
                Ok(true) => continue,
                Ok(false) => {}
                Err(e) => tracing::warn!(error = %e, "pipeline queue pass stopped"),
            }
            tokio::time::sleep(Duration::from_secs(2)).await;
        }
    });
}
