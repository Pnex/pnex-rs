//! Scheduler with dependencies (data-pipelines.md D575): a build runs a target and
//! what it depends on, one step at a time, with the state in PostgreSQL so that
//! another pod carries on. Pipelines run in the worker, imports in the server; a
//! failed step blocks what depends on it, which keeps its last valid version.
//!
//! The same tick warns about stale datasets (D576) and publishes what was committed
//! (D578): the worker has neither Valkey nor notifications (D573).

use std::time::Duration;

use pnex_core::err_codes;
use pnex_core::pipelines::{
    dataset_channel, find_cycle, next_step, plan, Build, BuildState, BuildStep, BuildStepState,
    BuildTargetKind, DatasetEvent, GraphNode, PipelineDocument, PlanError, CHANGED_IDS_MAX,
    DATASET_COMMITTED, DATASET_SYNCED,
};
use sea_orm::{
    ConnectionTrait, DatabaseBackend, DatabaseConnection, DbErr, QueryResult, Statement,
};
use serde_json::{json, Value};
use uuid::Uuid;

use crate::models::_entities::sea_orm_active_enums::OrgMemberRole;
use crate::services::artifact_store::S3Store;
use crate::services::ontology::imports;

const TICK_EVERY: Duration = Duration::from_secs(15);
/// A step that never ends, usually a queue no worker takes.
const STEP_TIMEOUT: &str = "12 hours";
/// A Valkey frame has no replay: older commits are not announced late.
const PUBLISH_WITHIN: &str = "1 hour";
const PUBLISH_BATCH: i64 = 500;
const DUE_BATCH: i64 = 20;
/// Same as the versions of a table.
const BUILDS_KEPT: &str = "90 days";
const VALKEY_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Debug)]
pub enum BuildError {
    NotFound,
    TargetNotFound,
    Plan(PlanError),
    Db(DbErr),
}

impl From<DbErr> for BuildError {
    fn from(e: DbErr) -> Self {
        Self::Db(e)
    }
}

fn stmt(sql: &str, values: Vec<sea_orm::Value>) -> Statement {
    Statement::from_sql_and_values(DatabaseBackend::Postgres, sql, values)
}

fn parse_steps(v: Value) -> Result<Vec<BuildStep>, DbErr> {
    serde_json::from_value(v).map_err(|e| DbErr::Custom(format!("build steps: {e}")))
}

/// The pipelines of an org as the scheduler sees them. What a document writes and
/// reads itself is no dependency.
async fn pipeline_nodes(db: &impl ConnectionTrait, org_id: i64) -> Result<Vec<GraphNode>, DbErr> {
    let rows = db
        .query_all_raw(stmt(
            "SELECT id, name, document FROM pipelines WHERE org_id = $1 ORDER BY name",
            vec![org_id.into()],
        ))
        .await?;
    let mut out = Vec::with_capacity(rows.len());
    for r in &rows {
        let document: String = r.try_get("", "document")?;
        let id: Uuid = r.try_get("", "id")?;
        // Checked when saved: a document of an older format just has no edges.
        let doc: Option<PipelineDocument> = serde_saphyr::from_str(&document).ok();
        out.push(pipeline_node(id, r.try_get("", "name")?, doc.as_ref()));
    }
    Ok(out)
}

pub(crate) fn pipeline_node(id: Uuid, name: String, doc: Option<&PipelineDocument>) -> GraphNode {
    let writes: Vec<String> = doc
        .into_iter()
        .flat_map(|d| d.transforms.iter().map(|t| t.output.clone()))
        .collect();
    let mut reads: Vec<String> = doc
        .into_iter()
        .flat_map(|d| d.transforms.iter())
        .flat_map(|t| t.inputs.values().cloned())
        .filter(|d| !writes.contains(d))
        .collect();
    reads.sort();
    reads.dedup();
    GraphNode {
        kind: BuildTargetKind::Pipeline,
        id: id.to_string(),
        name,
        runnable: doc.is_some(),
        reads,
        writes,
        after: Vec::new(),
        key: None,
    }
}

/// Everything of the org a schedule can chain.
pub async fn graph(db: &impl ConnectionTrait, org_id: i64) -> Result<Vec<GraphNode>, DbErr> {
    let mut nodes = pipeline_nodes(db, org_id).await?;
    nodes.extend(imports::graph_nodes(db, org_id).await?);
    Ok(nodes)
}

/// The cycle a new version would close, refused at save (D575).
pub async fn cycle_with(
    db: &impl ConnectionTrait,
    org_id: i64,
    candidate: GraphNode,
) -> Result<Option<String>, DbErr> {
    let mut nodes = graph(db, org_id).await?;
    match nodes
        .iter_mut()
        .find(|n| n.kind == candidate.kind && n.id == candidate.id)
    {
        Some(n) => *n = candidate,
        None => nodes.push(candidate),
    }
    Ok(find_cycle(&nodes))
}

/// The steps a build of this target would run, in order.
pub async fn steps_of(
    db: &DatabaseConnection,
    org_id: i64,
    kind: BuildTargetKind,
    target: Uuid,
) -> Result<(String, Vec<BuildStep>), BuildError> {
    let nodes = graph(db, org_id).await?;
    let id = target.to_string();
    let at = nodes
        .iter()
        .position(|n| n.kind == kind && n.id == id)
        .ok_or(BuildError::TargetNotFound)?;
    let steps = plan(&nodes, at).map_err(BuildError::Plan)?;
    Ok((nodes[at].name.clone(), steps))
}

pub fn plan_refusal(e: &PlanError) -> (&'static str, Value) {
    match e {
        PlanError::Cycle { through } => (err_codes::BUILD_CYCLE, json!({ "through": through })),
        PlanError::Missing { name, after } => (
            err_codes::BUILD_DEPENDENCY_MISSING,
            json!({ "name": name, "after": after }),
        ),
        PlanError::TooLong => (
            err_codes::BUILD_TOO_LONG,
            json!({ "max": pnex_core::pipelines::BUILD_STEPS_MAX }),
        ),
    }
}

const BUILD_SELECT: &str =
    "SELECT id, target_kind, target_id, target_name, trigger, state, again, \
    code, args, steps, \
    to_char(created_at AT TIME ZONE 'UTC', 'YYYY-MM-DD\"T\"HH24:MI:SS.MS\"Z\"') AS created_at, \
    to_char(finished_at AT TIME ZONE 'UTC', 'YYYY-MM-DD\"T\"HH24:MI:SS.MS\"Z\"') AS finished_at \
    FROM data_builds";

fn kind_of(s: &str) -> BuildTargetKind {
    match s {
        "pipeline" => BuildTargetKind::Pipeline,
        _ => BuildTargetKind::Import,
    }
}

fn build_of(r: &QueryResult) -> Result<Build, DbErr> {
    let state: String = r.try_get("", "state")?;
    let kind: String = r.try_get("", "target_kind")?;
    Ok(Build {
        id: r.try_get::<Uuid>("", "id")?.to_string(),
        target_kind: kind_of(&kind),
        target_id: r.try_get::<Uuid>("", "target_id")?.to_string(),
        target_name: r.try_get("", "target_name")?,
        trigger: r.try_get("", "trigger")?,
        state: match state.as_str() {
            "running" => BuildState::Running,
            "done" => BuildState::Done,
            _ => BuildState::Failed,
        },
        again: r.try_get("", "again")?,
        code: r.try_get("", "code")?,
        args: r.try_get("", "args")?,
        steps: parse_steps(r.try_get("", "steps")?)?,
        created_at: r.try_get("", "created_at")?,
        finished_at: r.try_get("", "finished_at")?,
    })
}

pub async fn get(db: &DatabaseConnection, org_id: i64, id: Uuid) -> Result<Build, BuildError> {
    let sql = format!("{BUILD_SELECT} WHERE org_id = $1 AND id = $2");
    let row = db
        .query_one_raw(stmt(&sql, vec![org_id.into(), id.into()]))
        .await?
        .ok_or(BuildError::NotFound)?;
    Ok(build_of(&row)?)
}

pub async fn list(
    db: &DatabaseConnection,
    org_id: i64,
    limit: i64,
    offset: i64,
) -> Result<(i64, Vec<Build>), BuildError> {
    let total = db
        .query_one_raw(stmt(
            "SELECT count(*) AS n FROM data_builds WHERE org_id = $1",
            vec![org_id.into()],
        ))
        .await?
        .map_or(Ok(0), |r| r.try_get("", "n"))?;
    let sql =
        format!("{BUILD_SELECT} WHERE org_id = $1 ORDER BY created_at DESC LIMIT $2 OFFSET $3");
    let rows = db
        .query_all_raw(stmt(&sql, vec![org_id.into(), limit.into(), offset.into()]))
        .await?;
    Ok((total, rows.iter().map(build_of).collect::<Result<_, _>>()?))
}

/// Starts a build, or asks the running one of this target for another pass.
pub async fn request(
    db: &DatabaseConnection,
    org_id: i64,
    kind: BuildTargetKind,
    target: Uuid,
    trigger: &str,
    requested_by: Option<i64>,
) -> Result<Build, BuildError> {
    let (name, steps) = steps_of(db, org_id, kind, target).await?;
    let steps = serde_json::to_value(&steps).unwrap_or_else(|_| json!([]));
    // The running one can end between the two statements: a second try starts anew.
    for _ in 0..2 {
        let row = db
            .query_one_raw(stmt(
                "INSERT INTO data_builds (org_id, target_kind, target_id, target_name, trigger, \
                   requested_by, steps) VALUES ($1, $2, $3, $4, $5, $6, $7) \
                 ON CONFLICT (target_kind, target_id) WHERE state = 'running' DO NOTHING \
                 RETURNING id",
                vec![
                    org_id.into(),
                    kind.wire().into(),
                    target.into(),
                    name.clone().into(),
                    trigger.into(),
                    requested_by.into(),
                    steps.clone().into(),
                ],
            ))
            .await?;
        let row = match row {
            Some(r) => Some(r),
            None => {
                db.query_one_raw(stmt(
                    "UPDATE data_builds SET again = true \
                     WHERE target_kind = $1 AND target_id = $2 AND state = 'running' RETURNING id",
                    vec![kind.wire().into(), target.into()],
                ))
                .await?
            }
        };
        if let Some(r) = row {
            return get(db, org_id, r.try_get("", "id")?).await;
        }
    }
    Err(BuildError::NotFound)
}

/// A schedule that cannot start leaves a failed build the org is told about (D303:
/// never silent), and nothing runs.
async fn refuse(
    db: &DatabaseConnection,
    org_id: i64,
    kind: BuildTargetKind,
    target: Uuid,
    name: &str,
    code: &str,
    args: Value,
) -> Result<(), DbErr> {
    db.execute_raw(stmt(
        "INSERT INTO data_builds (org_id, target_kind, target_id, target_name, trigger, state, \
           code, args, finished_at) VALUES ($1, $2, $3, $4, 'schedule', 'failed', $5, $6, now())",
        vec![
            org_id.into(),
            kind.wire().into(),
            target.into(),
            name.into(),
            code.into(),
            args.clone().into(),
        ],
    ))
    .await?;
    notify_failed_build(db, org_id, name, code, &args).await;
    Ok(())
}

/// The sweep of due imports goes through a build, like a pipeline.
pub async fn fire(
    db: &DatabaseConnection,
    org_id: i64,
    kind: BuildTargetKind,
    target: Uuid,
    name: &str,
    author: Option<i64>,
) -> Result<(), DbErr> {
    match request(db, org_id, kind, target, "schedule", author).await {
        Ok(b) => {
            tracing::info!(build = %b.id, target = %target, "scheduled build requested");
            Ok(())
        }
        Err(BuildError::Plan(e)) => {
            let (code, args) = plan_refusal(&e);
            refuse(db, org_id, kind, target, name, code, args).await
        }
        Err(BuildError::Db(e)) => Err(e),
        Err(e) => {
            tracing::warn!(target = %target, error = ?e, "scheduled build not requested");
            Ok(())
        }
    }
}

/// The rights a build needs, whatever starts it (R2): those of each kind of step.
pub fn rights_of(steps: &[BuildStep]) -> Vec<&'static str> {
    let mut needed = vec!["pipelines:run"];
    if steps.iter().any(|s| s.kind == BuildTargetKind::Pipeline) {
        needed.extend(["datasets:read", "datasets:write"]);
    }
    if steps.iter().any(|s| s.kind == BuildTargetKind::Import) {
        needed.push("ontology:write");
    }
    needed
}

/// R2: whoever set a schedule must still hold what its steps need. Only the managed
/// role of the membership counts here: rights from an assigned role need a person.
async fn may_run(
    db: &DatabaseConnection,
    org_id: i64,
    user: Option<i64>,
    steps: &[BuildStep],
) -> Result<bool, DbErr> {
    use crate::models::_entities::organization_members as om;
    use sea_orm::{ColumnTrait, EntityTrait, QueryFilter};
    let Some(user) = user else {
        return Ok(false);
    };
    let role: Option<OrgMemberRole> = om::Entity::find()
        .filter(om::Column::OrgId.eq(org_id))
        .filter(om::Column::UserId.eq(user))
        .one(db)
        .await?
        .map(|m| m.role);
    Ok(role.is_some_and(|r| {
        rights_of(steps)
            .iter()
            .all(|p| crate::services::iam::authorize(&r, p).allowed())
    }))
}

async fn fire_due_pipelines(db: &DatabaseConnection) -> Result<(), DbErr> {
    let due = db
        .query_all_raw(stmt(
            "SELECT id, org_id, name, recurrence, scheduled_by FROM pipelines \
             WHERE next_run_at IS NOT NULL AND next_run_at <= now() \
             ORDER BY next_run_at LIMIT $1",
            vec![DUE_BATCH.into()],
        ))
        .await?;
    for r in &due {
        let id: Uuid = r.try_get("", "id")?;
        let org_id: i64 = r.try_get("", "org_id")?;
        let name: String = r.try_get("", "name")?;
        let recurrence: Option<Value> = r.try_get("", "recurrence")?;
        let author: Option<i64> = r.try_get("", "scheduled_by")?;
        // Moved first: a build that cannot start is not retried in a loop.
        db.execute_raw(stmt(
            "UPDATE pipelines SET next_run_at = $2 WHERE id = $1",
            vec![id.into(), imports::next_run(recurrence.as_ref()).into()],
        ))
        .await?;
        let kind = BuildTargetKind::Pipeline;
        // An impossible plan is refused by `fire`, which says why.
        let steps = match steps_of(db, org_id, kind, id).await {
            Ok((_, steps)) => steps,
            Err(_) => Vec::new(),
        };
        if may_run(db, org_id, author, &steps).await? {
            fire(db, org_id, kind, id, &name, author).await?;
        } else {
            let code = err_codes::BUILD_AUTHOR_FORBIDDEN;
            refuse(db, org_id, kind, id, &name, code, Value::Null).await?;
        }
    }
    Ok(())
}

/// Where a started step stands.
async fn poll_step(
    db: &DatabaseConnection,
    step: &BuildStep,
) -> Result<Option<(BuildStepState, Option<String>)>, DbErr> {
    let Some(run) = step.run_id.as_deref().and_then(|r| r.parse::<Uuid>().ok()) else {
        return Ok(Some((BuildStepState::Pending, None)));
    };
    let (sql, code_column) = match step.kind {
        BuildTargetKind::Pipeline => (
            "SELECT state, code FROM pipeline_jobs WHERE id = $1",
            "code",
        ),
        BuildTargetKind::Import => (
            "SELECT state, state_code FROM object_import_runs WHERE id = $1",
            "state_code",
        ),
    };
    let Some(row) = db.query_one_raw(stmt(sql, vec![run.into()])).await? else {
        // Its pipeline or import was deleted during the build.
        return Ok(Some((
            BuildStepState::Failed,
            Some(err_codes::BUILD_NOT_FOUND.to_string()),
        )));
    };
    let state: String = row.try_get("", "state")?;
    let code: Option<String> = row.try_get("", code_column)?;
    Ok(match state.as_str() {
        "queued" | "running" => None,
        "done" | "applied" => Some((BuildStepState::Done, None)),
        // A run stopped at a preview waits for a person: what follows must not go on.
        "preview" => Some((
            BuildStepState::Failed,
            Some(code.unwrap_or_else(|| err_codes::IMPORT_PREVIEW_REQUIRED.to_string())),
        )),
        _ => Some((BuildStepState::Failed, code)),
    })
}

/// Starts a step. `Ok(false)`: not now (a run of it is active, the server is stopping).
async fn start_step(
    db: &DatabaseConnection,
    store: Option<&S3Store>,
    org_id: i64,
    author: Option<i64>,
    step: &mut BuildStep,
) -> Result<bool, DbErr> {
    let fail = |step: &mut BuildStep, code: &str| {
        step.state = BuildStepState::Failed;
        step.code = Some(code.to_string());
        Ok(true)
    };
    let Ok(id) = step.id.parse::<Uuid>() else {
        return fail(step, err_codes::BUILD_NOT_FOUND);
    };
    match step.kind {
        BuildTargetKind::Pipeline => {
            let row = db
                .query_one_raw(stmt(
                    "INSERT INTO pipeline_jobs (org_id, pipeline_id, version, trigger) \
                     SELECT org_id, id, version, 'build' FROM pipelines \
                     WHERE org_id = $1 AND id = $2 \
                     ON CONFLICT (pipeline_id) WHERE state IN ('queued', 'running') DO NOTHING \
                     RETURNING id",
                    vec![org_id.into(), id.into()],
                ))
                .await?;
            match row {
                Some(r) => {
                    step.run_id = Some(r.try_get::<Uuid>("", "id")?.to_string());
                    step.state = BuildStepState::Running;
                    Ok(true)
                }
                None => {
                    let exists = db
                        .query_one_raw(stmt(
                            "SELECT 1 AS x FROM pipelines WHERE org_id = $1 AND id = $2",
                            vec![org_id.into(), id.into()],
                        ))
                        .await?
                        .is_some();
                    if exists {
                        Ok(false)
                    } else {
                        fail(step, err_codes::PIPELINE_NOT_FOUND)
                    }
                }
            }
        }
        BuildTargetKind::Import => {
            let active = db
                .query_one_raw(stmt(
                    "SELECT 1 AS x FROM object_import_runs \
                     WHERE import_id = $1 AND state = 'running'",
                    vec![id.into()],
                ))
                .await?
                .is_some();
            if active {
                return Ok(false);
            }
            match imports::start(db, store.cloned(), org_id, id, author, "schedule").await {
                Ok(run) => {
                    step.run_id = Some(run.id);
                    step.state = BuildStepState::Running;
                    Ok(true)
                }
                Err(e) if imports::stopping_refusal(&e) => Ok(false),
                Err(e) => fail(step, imports::error_code(&e)),
            }
        }
    }
}

async fn save_steps(
    db: &DatabaseConnection,
    id: Uuid,
    steps: &[BuildStep],
    moved: bool,
) -> Result<(), DbErr> {
    let steps = serde_json::to_value(steps).unwrap_or_else(|_| json!([]));
    let at = if moved {
        "now()"
    } else {
        "coalesce(step_started_at, now())"
    };
    db.execute_raw(stmt(
        &format!("UPDATE data_builds SET steps = $2, step_started_at = {at} WHERE id = $1"),
        vec![id.into(), steps.into()],
    ))
    .await?;
    Ok(())
}

/// Moves one build as far as it can go now.
async fn advance(
    db: &DatabaseConnection,
    store: Option<&S3Store>,
    row: &QueryResult,
) -> Result<(), DbErr> {
    let id: Uuid = row.try_get("", "id")?;
    let org_id: i64 = row.try_get("", "org_id")?;
    let author: Option<i64> = row.try_get("", "requested_by")?;
    let late: bool = row.try_get("", "late")?;
    let mut steps = parse_steps(row.try_get("", "steps")?)?;

    if let Some(i) = steps
        .iter()
        .position(|s| s.state == BuildStepState::Running)
    {
        match poll_step(db, &steps[i]).await? {
            None if !late => return Ok(()),
            None => {
                steps[i].state = BuildStepState::Failed;
                steps[i].code = Some(err_codes::BUILD_TIMEOUT.to_string());
            }
            Some((state, code)) => {
                steps[i].state = state;
                steps[i].code = code;
                if steps[i].retry_if_lost() {
                    tracing::info!(build = %id, step = %steps[i].name, "run lost with its server: starting again");
                }
            }
        }
        save_steps(db, id, &steps, true).await?;
    }
    if let Some(i) = next_step(&mut steps) {
        // Nobody is connected behind a schedule: its author's rights are read before each
        // step, or someone demoted would keep writing tables with every recurrence.
        let trigger: String = row.try_get("", "trigger")?;
        if trigger == "schedule" && !may_run(db, org_id, author, &steps).await? {
            steps[i].state = BuildStepState::Failed;
            steps[i].code = Some(err_codes::BUILD_AUTHOR_FORBIDDEN.to_string());
            return save_steps(db, id, &steps, true).await;
        }
        let moved = start_step(db, store, org_id, author, &mut steps[i]).await?;
        if !moved && late {
            steps[i].state = BuildStepState::Failed;
            steps[i].code = Some(err_codes::BUILD_TIMEOUT.to_string());
            save_steps(db, id, &steps, true).await?;
            return Ok(());
        }
        return save_steps(db, id, &steps, moved).await;
    }
    finish(db, id, org_id, row, &steps).await
}

async fn finish(
    db: &DatabaseConnection,
    id: Uuid,
    org_id: i64,
    row: &QueryResult,
    steps: &[BuildStep],
) -> Result<(), DbErr> {
    use BuildStepState::{Blocked, Failed};
    let failed = steps.iter().find(|s| s.state == Failed);
    let ended = steps.iter().any(|s| matches!(s.state, Failed | Blocked));
    let code = failed.and_then(|s| s.code.clone());
    let args = failed.map(|s| json!({ "step": s.name }));
    let state = if ended { "failed" } else { "done" };
    let done = db
        .execute_raw(stmt(
            "UPDATE data_builds SET state = $2, code = $3, args = $4, \
               steps = $5, finished_at = now() WHERE id = $1 AND state = 'running'",
            vec![
                id.into(),
                state.into(),
                code.clone().into(),
                args.clone().into(),
                serde_json::to_value(steps).unwrap_or_default().into(),
            ],
        ))
        .await?;
    if done.rows_affected() == 0 {
        return Ok(());
    }
    let name: String = row.try_get("", "target_name")?;
    if ended {
        let (code, args) = (
            code.as_deref().unwrap_or_default(),
            args.unwrap_or_default(),
        );
        notify_failed_build(db, org_id, &name, code, &args).await;
    }
    // The request that came during this build gets its own pass.
    if row.try_get::<bool>("", "again").unwrap_or(false)
        || again_since_read(db, id).await.unwrap_or(false)
    {
        let kind: String = row.try_get("", "target_kind")?;
        let target: Uuid = row.try_get("", "target_id")?;
        let trigger: String = row.try_get("", "trigger")?;
        let author: Option<i64> = row.try_get("", "requested_by")?;
        if let Err(e) = request(db, org_id, kind_of(&kind), target, &trigger, author).await {
            tracing::warn!(target = %target, error = ?e, "next build not requested");
        }
    }
    Ok(())
}

async fn again_since_read(db: &DatabaseConnection, id: Uuid) -> Result<bool, DbErr> {
    db.query_one_raw(stmt(
        "SELECT again FROM data_builds WHERE id = $1",
        vec![id.into()],
    ))
    .await?
    .map_or(Ok(false), |r| r.try_get("", "again"))
}

/// In-app notice to an org; the channel is the org's first websocket.
pub(crate) async fn notify_org(
    db: &impl ConnectionTrait,
    org_id: i64,
    source: &str,
    msg: pnex_notify::Message,
) {
    use crate::models::_entities::notify_channels;
    use sea_orm::{ColumnTrait, EntityTrait, QueryFilter, QueryOrder};
    let channel = notify_channels::Entity::find()
        .filter(notify_channels::Column::OrgId.eq(org_id))
        .filter(notify_channels::Column::Kind.eq("websocket"))
        .order_by_asc(notify_channels::Column::Id)
        .one(db)
        .await
        .ok()
        .flatten();
    if let Some(ch) = channel {
        crate::services::notify::deliver_in_app(
            pnex_core::NotifyDeliveryEntry {
                org_id,
                channel_id: ch.id,
                source: source.into(),
                ..Default::default()
            },
            &msg,
        );
    }
}

async fn notify_failed_build(
    db: &impl ConnectionTrait,
    org_id: i64,
    name: &str,
    code: &str,
    args: &Value,
) {
    tracing::info!(org = org_id, target = name, code, "build failed");
    let msg = pnex_notify::Message {
        subject: Some(format!("Data refresh of {name} failed")),
        body: format!(
            "The refresh of {name} stopped ({code}). What depends on it keeps its last valid version."
        ),
        meta: json!({ "kind": "build-failed", "target": name, "code": code, "args": args }),
    };
    notify_org(db, org_id, "pipeline", msg).await;
}

/// Failed runs outside a build: the build's own notice covers the others.
async fn notify_failed_jobs(db: &DatabaseConnection) -> Result<(), DbErr> {
    let rows = db
        .query_all_raw(stmt(
            "UPDATE pipeline_jobs j SET notified_at = now() FROM pipelines p \
             WHERE p.id = j.pipeline_id AND j.state = 'failed' AND j.notified_at IS NULL \
             RETURNING j.org_id, j.code, j.args, j.trigger, p.name",
            vec![],
        ))
        .await?;
    for r in &rows {
        if r.try_get::<String>("", "trigger")? == "build" {
            continue;
        }
        let org_id: i64 = r.try_get("", "org_id")?;
        let name: String = r.try_get("", "name")?;
        let code: Option<String> = r.try_get("", "code")?;
        let args: Option<Value> = r.try_get("", "args")?;
        let msg = pnex_notify::Message {
            subject: Some(format!("Pipeline {name} failed")),
            body: format!(
                "A run of the pipeline {name} failed ({}). The previous versions stay current.",
                code.as_deref().unwrap_or("unknown")
            ),
            meta: json!({ "kind": "pipeline-failed", "pipeline": name, "code": code, "args": args }),
        };
        notify_org(db, org_id, "pipeline", msg).await;
    }
    Ok(())
}

/// Warns once for each time a dataset goes without a new version for too long (D576).
/// No run happens then: only a sweep can see it.
async fn notify_stale(db: &DatabaseConnection) -> Result<(), DbErr> {
    let rows = db
        .query_all_raw(stmt(
            "WITH stale AS ( \
               SELECT d.id FROM datasets d \
               LEFT JOIN LATERAL ( \
                 SELECT max(t.committed_at) AS at FROM dataset_transactions t \
                 WHERE t.dataset_id = d.id AND t.state = 'committed') v ON true \
               WHERE d.max_age_hours IS NOT NULL \
               AND coalesce(v.at, d.created_at) < now() - make_interval(hours => d.max_age_hours) \
               AND (d.stale_notified_at IS NULL \
                 OR d.stale_notified_at < coalesce(v.at, d.created_at)) \
             ) \
             UPDATE datasets d SET stale_notified_at = now() FROM stale WHERE d.id = stale.id \
             RETURNING d.org_id, d.name, d.max_age_hours",
            vec![],
        ))
        .await?;
    for r in &rows {
        let org_id: i64 = r.try_get("", "org_id")?;
        let name: String = r.try_get("", "name")?;
        let hours: i32 = r.try_get("", "max_age_hours")?;
        let msg = pnex_notify::Message {
            subject: Some(format!("Table {name} is stale")),
            body: format!("The table {name} has had no new version for over {hours} hours."),
            meta: json!({ "kind": "dataset-stale", "dataset": name, "max_age_hours": hours }),
        };
        notify_org(db, org_id, "dataset", msg).await;
    }
    Ok(())
}

async fn send(conn: &mut redis::aio::ConnectionManager, org_id: i64, event: &DatasetEvent) -> bool {
    let Ok(frame) = serde_json::to_string(event) else {
        return false;
    };
    let op = redis::AsyncCommands::publish::<_, _, i64>(conn, dataset_channel(org_id), frame);
    match tokio::time::timeout(VALKEY_TIMEOUT, op).await {
        Ok(Ok(_)) => true,
        Ok(Err(e)) => {
            tracing::warn!(error = %e, "dataset event: valkey error");
            false
        }
        Err(_) => {
            tracing::warn!("dataset event: valkey timeout");
            false
        }
    }
}

/// Announces new versions and applied syncs to flows (D578). Kept in place until
/// sent, so a pod lost between commit and publication loses no event, and a commit
/// from the worker, which has no Valkey, gets one too.
pub async fn publish_events(
    db: &DatabaseConnection,
    conn: &mut redis::aio::ConnectionManager,
) -> Result<(), DbErr> {
    let late = format!("now() - interval '{PUBLISH_WITHIN}'");
    let now_ms = chrono::Utc::now().timestamp_millis();
    db.execute_raw(stmt(
        &format!(
            "UPDATE dataset_transactions SET published_at = now() \
             WHERE state = 'committed' AND published_at IS NULL AND committed_at < {late}"
        ),
        vec![],
    ))
    .await?;
    let commits = db
        .query_all_raw(stmt(
            "SELECT t.id, t.org_id, t.rows, t.bytes, d.id AS dataset_id, d.name \
             FROM dataset_transactions t JOIN datasets d ON d.id = t.dataset_id \
             WHERE t.state = 'committed' AND t.published_at IS NULL ORDER BY t.seq LIMIT $1",
            vec![PUBLISH_BATCH.into()],
        ))
        .await?;
    for r in &commits {
        let id: Uuid = r.try_get("", "id")?;
        let event = DatasetEvent {
            event: DATASET_COMMITTED.into(),
            dataset_id: r.try_get::<Uuid>("", "dataset_id")?.to_string(),
            dataset: r.try_get("", "name")?,
            reference: id.to_string(),
            counts: json!({
                "rows": r.try_get::<i64>("", "rows")?,
                "bytes": r.try_get::<i64>("", "bytes")?,
            }),
            changed: Vec::new(),
            ts_ms: now_ms,
        };
        if !send(conn, r.try_get("", "org_id")?, &event).await {
            return Ok(());
        }
        db.execute_raw(stmt(
            "UPDATE dataset_transactions SET published_at = now() WHERE id = $1",
            vec![id.into()],
        ))
        .await?;
    }
    // A sync is an import reading a table, or filling its own.
    db.execute_raw(stmt(
        &format!(
            "UPDATE object_import_runs r SET published_at = now() FROM object_imports i \
             WHERE i.id = r.import_id AND r.state = 'applied' AND r.published_at IS NULL \
             AND (r.finished_at < {late} OR NOT EXISTS ( \
               SELECT 1 FROM datasets d WHERE d.org_id = r.org_id AND (d.import_id = i.id \
                 OR (i.source = 'dataset' AND i.source_url = d.id::text))))"
        ),
        vec![],
    ))
    .await?;
    let syncs = db
        .query_all_raw(stmt(
            "SELECT r.id, r.org_id, r.import_id, r.number, r.counts, d.id AS dataset_id, d.name \
             FROM object_import_runs r JOIN object_imports i ON i.id = r.import_id \
             JOIN datasets d ON d.org_id = r.org_id AND (d.import_id = i.id \
               OR (i.source = 'dataset' AND i.source_url = d.id::text)) \
             WHERE r.state = 'applied' AND r.published_at IS NULL \
             ORDER BY r.finished_at LIMIT $1",
            vec![PUBLISH_BATCH.into()],
        ))
        .await?;
    for r in &syncs {
        let id: Uuid = r.try_get("", "id")?;
        let org_id: i64 = r.try_get("", "org_id")?;
        let import: Uuid = r.try_get("", "import_id")?;
        let number: i32 = r.try_get("", "number")?;
        let changed = db
            .query_all_raw(stmt(
                "SELECT id FROM objects WHERE org_id = $1 AND source_ref = $2 LIMIT $3",
                vec![
                    org_id.into(),
                    format!("import:{import}@{number}").into(),
                    CHANGED_IDS_MAX.into(),
                ],
            ))
            .await?
            .iter()
            .filter_map(|o| o.try_get::<Uuid>("", "id").ok().map(|u| u.to_string()))
            .collect();
        let event = DatasetEvent {
            event: DATASET_SYNCED.into(),
            dataset_id: r.try_get::<Uuid>("", "dataset_id")?.to_string(),
            dataset: r.try_get("", "name")?,
            reference: format!("{import}@{number}"),
            counts: r.try_get("", "counts")?,
            changed,
            ts_ms: now_ms,
        };
        if !send(conn, org_id, &event).await {
            return Ok(());
        }
        db.execute_raw(stmt(
            "UPDATE object_import_runs SET published_at = now() WHERE id = $1",
            vec![id.into()],
        ))
        .await?;
    }
    Ok(())
}

/// One round: due schedules, running builds, then notices. Public for the tests.
pub async fn tick(db: &DatabaseConnection, store: Option<&S3Store>) -> Result<(), DbErr> {
    fire_due_pipelines(db).await?;
    let running = db
        .query_all_raw(stmt(
            &format!(
                "SELECT id, org_id, target_kind, target_id, target_name, trigger, requested_by, \
                 again, steps, coalesce(step_started_at < now() - interval '{STEP_TIMEOUT}', false) \
                 AS late FROM data_builds WHERE state = 'running' ORDER BY created_at"
            ),
            vec![],
        ))
        .await?;
    for row in &running {
        // One build failing to move does not stop the others.
        if let Err(e) = advance(db, store, row).await {
            tracing::warn!(error = %e, "build not advanced");
        }
    }
    notify_failed_jobs(db).await?;
    notify_stale(db).await?;
    db.execute_raw(stmt(
        &format!("DELETE FROM data_builds WHERE finished_at < now() - interval '{BUILDS_KEPT}'"),
        vec![],
    ))
    .await?;
    Ok(())
}

pub fn spawn(ctx: &loco_rs::app::AppContext) {
    let db = ctx.db.clone();
    let config = ctx.config.clone();
    let store = crate::services::artifact_store::S3Config::from_config(&ctx.config)
        .store()
        .ok();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_secs(30)).await;
        loop {
            if !imports::stop::stopping()
                && crate::services::singleton::my_turn(&db, "data-builds", TICK_EVERY).await
            {
                if let Err(e) = tick(&db, store.as_ref()).await {
                    tracing::warn!("build tick failed: {e}");
                }
                if let Some(mut conn) = crate::services::shared_valkey::conn(&config).await {
                    if let Err(e) = publish_events(&db, &mut conn).await {
                        tracing::warn!("dataset events not published: {e}");
                    }
                }
            }
            tokio::time::sleep(TICK_EVERY).await;
        }
    });
}
