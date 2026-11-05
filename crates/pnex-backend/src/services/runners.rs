//! Runners and their job queue (agent-environments.md lot E0, worker-fabric.md
//! section 8 and 14): state lives in PostgreSQL; a runner polls out, holds a job by
//! lease, and a reaper gives back what a dead runner held.

use std::time::Duration;

use loco_rs::app::AppContext;
use pnex_core::runners::{
    JoinTokenCreated, RunnerClaimedJob, RunnerJobCreate, RunnerJobFinish, RunnerJobKind,
    RunnerJobOutcome, RunnerJobSpec, RunnerJobState, RunnerJobView, RunnerRegistered, RunnerView,
    JOIN_TOKEN_PREFIX, RUNNER_TOKEN_PREFIX,
};
use rand::RngExt;
use sea_orm::{
    ConnectionTrait, DatabaseBackend, DatabaseConnection, DbErr, QueryResult, Statement,
    TransactionTrait,
};
use serde_json::json;
use sha2::{Digest, Sha256};
use uuid::Uuid;

/// A runner renews every 30 s: four missed heartbeats make a dead one.
pub const LEASE_SECS: u32 = 120;
const JOIN_TOKEN_TTL: &str = "1 hour";
/// No compatible runner in that time: `expired`, never moved elsewhere (D393).
const QUEUE_TTL: &str = "1 hour";
const MAX_ATTEMPTS: i32 = 3;
const FINISHED_KEPT: &str = "90 days";
const REAP_EVERY: Duration = Duration::from_secs(30);

#[derive(Debug)]
pub enum RunnerError {
    NotFound,
    JoinTokenInvalid,
    /// The job is not held by this runner any more: it must drop its workload.
    NotHeld,
    Db(DbErr),
}

impl From<DbErr> for RunnerError {
    fn from(e: DbErr) -> Self {
        Self::Db(e)
    }
}

/// An authenticated runner: the org always comes from here (R1).
#[derive(Debug, Clone, Copy)]
pub struct RunnerPrincipal {
    pub id: Uuid,
    pub org_id: i64,
}

fn new_token(prefix: &str) -> String {
    let mut bytes = [0u8; 32];
    rand::rng().fill(&mut bytes);
    let hex: String = bytes.iter().map(|b| format!("{b:02x}")).collect();
    format!("{prefix}{hex}")
}

fn hash(token: &str) -> String {
    Sha256::digest(token.as_bytes())
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

fn stmt(sql: &str, values: Vec<sea_orm::Value>) -> Statement {
    Statement::from_sql_and_values(DatabaseBackend::Postgres, sql, values)
}

fn iso(r: &QueryResult, column: &str) -> Result<Option<String>, DbErr> {
    let at: Option<chrono::DateTime<chrono::Utc>> = r.try_get("", column)?;
    Ok(at.map(|t| t.to_rfc3339()))
}

fn strings(r: &QueryResult, column: &str) -> Result<Vec<String>, DbErr> {
    let v: serde_json::Value = r.try_get("", column)?;
    serde_json::from_value(v).map_err(|e| DbErr::Custom(format!("{column}: {e}")))
}

fn runner_view(r: &QueryResult) -> Result<RunnerView, DbErr> {
    Ok(RunnerView {
        id: r.try_get::<Uuid>("", "id")?.to_string(),
        name: r.try_get("", "name")?,
        capabilities: strings(r, "capabilities")?,
        online: r.try_get("", "online")?,
        last_seen_at: iso(r, "last_seen_at")?,
        created_at: iso(r, "created_at")?.unwrap_or_default(),
    })
}

fn spec_of(r: &QueryResult) -> Result<RunnerJobSpec, DbErr> {
    let v: serde_json::Value = r.try_get("", "spec")?;
    serde_json::from_value(v).map_err(|e| DbErr::Custom(format!("spec: {e}")))
}

fn kind_of(r: &QueryResult) -> Result<RunnerJobKind, DbErr> {
    let kind: String = r.try_get("", "kind")?;
    RunnerJobKind::parse(&kind).ok_or_else(|| DbErr::Custom(format!("job kind {kind}")))
}

fn job_view(r: &QueryResult) -> Result<RunnerJobView, DbErr> {
    let state: String = r.try_get("", "state")?;
    let outcome: Option<String> = r.try_get("", "outcome")?;
    Ok(RunnerJobView {
        id: r.try_get::<Uuid>("", "id")?.to_string(),
        kind: kind_of(r)?,
        state: RunnerJobState::parse(&state)
            .ok_or_else(|| DbErr::Custom(format!("job state {state}")))?,
        spec: spec_of(r)?,
        required_capabilities: strings(r, "required_capabilities")?,
        runner_id: r
            .try_get::<Option<Uuid>>("", "runner_id")?
            .map(|u| u.to_string()),
        attempts: r.try_get("", "attempts")?,
        outcome: outcome.as_deref().and_then(RunnerJobOutcome::parse),
        exit_code: r.try_get("", "exit_code")?,
        created_at: iso(r, "created_at")?.unwrap_or_default(),
        started_at: iso(r, "started_at")?,
        finished_at: iso(r, "finished_at")?,
    })
}

const RUNNER_COLUMNS: &str = "id, name, capabilities, last_seen_at, created_at, \
     COALESCE(last_seen_at > now() - interval '2 minutes', false) AS online";

pub async fn create_join_token(
    db: &DatabaseConnection,
    org_id: i64,
    created_by: i64,
    name: &str,
) -> Result<JoinTokenCreated, RunnerError> {
    let token = new_token(JOIN_TOKEN_PREFIX);
    let row = db
        .query_one_raw(stmt(
            &format!(
                "INSERT INTO runner_join_tokens (org_id, name, token_hash, created_by, expires_at) \
                 VALUES ($1, $2, $3, $4, now() + interval '{JOIN_TOKEN_TTL}') RETURNING expires_at"
            ),
            vec![
                org_id.into(),
                name.into(),
                hash(&token).into(),
                created_by.into(),
            ],
        ))
        .await?
        .ok_or_else(|| DbErr::Custom("join token not inserted".into()))?;
    Ok(JoinTokenCreated {
        token,
        expires_at: iso(&row, "expires_at")?.unwrap_or_default(),
    })
}

/// Consumes the join token and enrols the runner, both or neither: two runners
/// racing on one token get one success.
pub async fn register(
    db: &DatabaseConnection,
    join_token: &str,
    capabilities: &[String],
) -> Result<RunnerRegistered, RunnerError> {
    let txn = db.begin().await?;
    let Some(row) = txn
        .query_one_raw(stmt(
            "UPDATE runner_join_tokens SET used_at = now() \
             WHERE token_hash = $1 AND used_at IS NULL AND expires_at > now() \
             RETURNING org_id, name",
            vec![hash(join_token).into()],
        ))
        .await?
    else {
        return Err(RunnerError::JoinTokenInvalid);
    };
    let org_id: i64 = row.try_get("", "org_id")?;
    let name: String = row.try_get("", "name")?;
    let token = new_token(RUNNER_TOKEN_PREFIX);
    let row = txn
        .query_one_raw(stmt(
            "INSERT INTO runners (org_id, name, token_hash, capabilities, last_seen_at) \
             VALUES ($1, $2, $3, $4, now()) RETURNING id",
            vec![
                org_id.into(),
                name.into(),
                hash(&token).into(),
                json!(capabilities).into(),
            ],
        ))
        .await?
        .ok_or_else(|| DbErr::Custom("runner not inserted".into()))?;
    let id: Uuid = row.try_get("", "id")?;
    txn.commit().await?;
    Ok(RunnerRegistered {
        runner_id: id.to_string(),
        token,
    })
}

/// The runner behind a token, seen now. `None` for an unknown or revoked token.
pub async fn authenticate(
    db: &DatabaseConnection,
    token: &str,
) -> Result<Option<RunnerPrincipal>, DbErr> {
    let Some(r) = db
        .query_one_raw(stmt(
            "UPDATE runners SET last_seen_at = now() WHERE token_hash = $1 RETURNING id, org_id",
            vec![hash(token).into()],
        ))
        .await?
    else {
        return Ok(None);
    };
    Ok(Some(RunnerPrincipal {
        id: r.try_get("", "id")?,
        org_id: r.try_get("", "org_id")?,
    }))
}

pub async fn list_runners(
    db: &DatabaseConnection,
    org_id: i64,
    limit: u64,
    offset: u64,
) -> Result<(i64, Vec<RunnerView>), DbErr> {
    let count = count_of(db, "runners", org_id).await?;
    let rows = db
        .query_all_raw(stmt(
            &format!(
                "SELECT {RUNNER_COLUMNS} FROM runners WHERE org_id = $1 \
                 ORDER BY created_at DESC LIMIT $2 OFFSET $3"
            ),
            vec![org_id.into(), (limit as i64).into(), (offset as i64).into()],
        ))
        .await?;
    Ok((
        count,
        rows.iter().map(runner_view).collect::<Result<_, _>>()?,
    ))
}

async fn count_of(db: &DatabaseConnection, table: &str, org_id: i64) -> Result<i64, DbErr> {
    let r = db
        .query_one_raw(stmt(
            &format!("SELECT count(*) AS n FROM {table} WHERE org_id = $1"),
            vec![org_id.into()],
        ))
        .await?
        .ok_or_else(|| DbErr::Custom("count".into()))?;
    r.try_get("", "n")
}

/// Revokes a runner: its token stops working, what it held goes back to the queue.
pub async fn delete_runner(
    db: &DatabaseConnection,
    org_id: i64,
    id: Uuid,
) -> Result<(), RunnerError> {
    let txn = db.begin().await?;
    txn.execute_raw(stmt(
        "UPDATE runner_jobs SET state = 'queued', runner_id = NULL, lease_expires_at = NULL \
         WHERE org_id = $1 AND runner_id = $2 AND state = 'running'",
        vec![org_id.into(), id.into()],
    ))
    .await?;
    let done = txn
        .execute_raw(stmt(
            "DELETE FROM runners WHERE org_id = $1 AND id = $2",
            vec![org_id.into(), id.into()],
        ))
        .await?;
    if done.rows_affected() == 0 {
        return Err(RunnerError::NotFound);
    }
    txn.commit().await?;
    Ok(())
}

pub async fn create_job<C: ConnectionTrait>(
    db: &C,
    org_id: i64,
    created_by: i64,
    body: &RunnerJobCreate,
) -> Result<RunnerJobView, DbErr> {
    let row = db
        .query_one_raw(stmt(
            &format!(
                "INSERT INTO runner_jobs \
                   (org_id, kind, spec, required_capabilities, created_by, queue_expires_at) \
                 VALUES ($1, $2, $3, $4, $5, now() + interval '{QUEUE_TTL}') RETURNING *"
            ),
            vec![
                org_id.into(),
                body.kind.as_str().into(),
                json!(body.spec).into(),
                json!(body.required_capabilities).into(),
                created_by.into(),
            ],
        ))
        .await?
        .ok_or_else(|| DbErr::Custom("job not inserted".into()))?;
    job_view(&row)
}

pub async fn list_jobs(
    db: &DatabaseConnection,
    org_id: i64,
    limit: u64,
    offset: u64,
) -> Result<(i64, Vec<RunnerJobView>), DbErr> {
    let count = count_of(db, "runner_jobs", org_id).await?;
    let rows = db
        .query_all_raw(stmt(
            "SELECT * FROM runner_jobs WHERE org_id = $1 \
             ORDER BY created_at DESC LIMIT $2 OFFSET $3",
            vec![org_id.into(), (limit as i64).into(), (offset as i64).into()],
        ))
        .await?;
    Ok((count, rows.iter().map(job_view).collect::<Result<_, _>>()?))
}

pub async fn get_job(
    db: &DatabaseConnection,
    org_id: i64,
    id: Uuid,
) -> Result<RunnerJobView, RunnerError> {
    let row = db
        .query_one_raw(stmt(
            "SELECT * FROM runner_jobs WHERE org_id = $1 AND id = $2",
            vec![org_id.into(), id.into()],
        ))
        .await?
        .ok_or(RunnerError::NotFound)?;
    Ok(job_view(&row)?)
}

/// Oldest job of the runner's org whose requirements its capabilities cover. Two
/// runners never take the same job (`SKIP LOCKED`).
pub async fn claim(
    db: &DatabaseConnection,
    runner: RunnerPrincipal,
) -> Result<Option<RunnerClaimedJob>, DbErr> {
    let Some(r) = db
        .query_one_raw(stmt(
            &format!(
                "UPDATE runner_jobs SET state = 'running', runner_id = $1, \
                   attempts = attempts + 1, started_at = now(), \
                   lease_expires_at = now() + interval '{LEASE_SECS} seconds' \
                 WHERE id = ( \
                   SELECT j.id FROM runner_jobs j, runners r \
                   WHERE r.id = $1 AND j.org_id = r.org_id AND j.org_id = $2 \
                     AND j.state = 'queued' AND j.queue_expires_at > now() \
                     AND j.required_capabilities <@ r.capabilities \
                   ORDER BY j.created_at LIMIT 1 FOR UPDATE OF j SKIP LOCKED \
                 ) RETURNING id, kind, spec"
            ),
            vec![runner.id.into(), runner.org_id.into()],
        ))
        .await?
    else {
        return Ok(None);
    };
    Ok(Some(RunnerClaimedJob {
        id: r.try_get::<Uuid>("", "id")?.to_string(),
        kind: kind_of(&r)?,
        spec: spec_of(&r)?,
        lease_secs: LEASE_SECS,
        build: None,
        scan: None,
        sandbox: None,
    }))
}

pub async fn heartbeat(
    db: &DatabaseConnection,
    runner: RunnerPrincipal,
    job: Uuid,
) -> Result<(), RunnerError> {
    let done = db
        .execute_raw(stmt(
            &format!(
                "UPDATE runner_jobs SET lease_expires_at = now() + interval '{LEASE_SECS} seconds' \
                 WHERE id = $1 AND org_id = $2 AND runner_id = $3 AND state = 'running'"
            ),
            vec![job.into(), runner.org_id.into(), runner.id.into()],
        ))
        .await?;
    if done.rows_affected() == 0 {
        return Err(RunnerError::NotHeld);
    }
    Ok(())
}

/// Ends a job held by this runner. A retry after a lost answer changes nothing and
/// succeeds; a job taken over by another runner answers `NotHeld`.
pub async fn finish(
    db: &DatabaseConnection,
    runner: RunnerPrincipal,
    job: Uuid,
    body: &RunnerJobFinish,
) -> Result<(), RunnerError> {
    let state = if body.outcome == RunnerJobOutcome::Succeeded {
        "succeeded"
    } else {
        "failed"
    };
    let done = db
        .execute_raw(stmt(
            "UPDATE runner_jobs SET state = $1, outcome = $2, exit_code = $3, \
               finished_at = now(), lease_expires_at = NULL \
             WHERE id = $4 AND org_id = $5 AND runner_id = $6 AND state = 'running'",
            vec![
                state.into(),
                body.outcome.as_str().into(),
                body.exit_code.into(),
                job.into(),
                runner.org_id.into(),
                runner.id.into(),
            ],
        ))
        .await?;
    if done.rows_affected() > 0 {
        return Ok(());
    }
    let already = db
        .query_one_raw(stmt(
            "SELECT 1 AS ok FROM runner_jobs WHERE id = $1 AND org_id = $2 AND runner_id = $3 \
             AND state IN ('succeeded', 'failed')",
            vec![job.into(), runner.org_id.into(), runner.id.into()],
        ))
        .await?;
    if already.is_some() {
        Ok(())
    } else {
        Err(RunnerError::NotHeld)
    }
}

/// What the reaper did in one round.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Reaped {
    pub requeued: u64,
    pub failed: u64,
    pub expired: u64,
}

/// Gives back jobs whose lease ran out, fails those that used up their attempts,
/// expires those no runner took, and drops old rows.
pub async fn reap(db: &DatabaseConnection) -> Result<Reaped, DbErr> {
    let failed = db
        .execute_raw(stmt(
            "UPDATE runner_jobs SET state = 'failed', outcome = $1, finished_at = now(), \
               lease_expires_at = NULL \
             WHERE state = 'running' AND lease_expires_at < now() AND attempts >= $2",
            vec![
                RunnerJobOutcome::LeaseExpired.as_str().into(),
                MAX_ATTEMPTS.into(),
            ],
        ))
        .await?
        .rows_affected();
    let requeued = db
        .execute_raw(stmt(
            &format!(
                "UPDATE runner_jobs SET state = 'queued', runner_id = NULL, \
                   lease_expires_at = NULL, queue_expires_at = now() + interval '{QUEUE_TTL}' \
                 WHERE state = 'running' AND lease_expires_at < now()"
            ),
            vec![],
        ))
        .await?
        .rows_affected();
    let expired = db
        .execute_raw(stmt(
            "UPDATE runner_jobs SET state = 'expired', finished_at = now() \
             WHERE state = 'queued' AND queue_expires_at < now()",
            vec![],
        ))
        .await?
        .rows_affected();
    db.execute_raw(stmt(
        &format!("DELETE FROM runner_jobs WHERE finished_at < now() - interval '{FINISHED_KEPT}'"),
        vec![],
    ))
    .await?;
    db.execute_raw(stmt(
        &format!(
            "DELETE FROM runner_join_tokens WHERE expires_at < now() - interval '{FINISHED_KEPT}'"
        ),
        vec![],
    ))
    .await?;
    Ok(Reaped {
        requeued,
        failed,
        expired,
    })
}

pub fn spawn_reaper(ctx: &AppContext) {
    let ctx = ctx.clone();
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(REAP_EVERY).await;
            if crate::services::singleton::my_turn(&ctx.db, "runner-reaper", REAP_EVERY).await {
                match reap(&ctx.db).await {
                    Ok(r) if r == Reaped::default() => {}
                    Ok(r) => tracing::info!(?r, "runner jobs reaped"),
                    Err(e) => tracing::warn!("runner reaper failed: {e}"),
                }
            }
        }
    });
}
