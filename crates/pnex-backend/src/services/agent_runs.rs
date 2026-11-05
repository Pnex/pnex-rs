//! Runs of hosted agents (agent-environments.md D731). A run is a `sandbox` job on the
//! active image of an environment. Its token is minted at each claim, stored hashed, and
//! opens the LLM gateway only while that job runs: a stop, the end of the job or a
//! takeover by another runner closes it at once.

use std::collections::BTreeMap;

use pnex_core::agent_runs::{create_error, AgentRun, AgentRunCreate, RUN_TOKEN_PREFIX};
use pnex_core::runners::{
    spec_error, RunnerJobCreate, RunnerJobKind, RunnerJobOutcome, RunnerJobSpec, RunnerJobState,
    RunnerSandboxInput, SandboxEgress,
};
use rand::RngExt;
use sea_orm::{
    ConnectionTrait, DatabaseBackend, DatabaseConnection, DbErr, QueryResult, Statement,
    TransactionTrait,
};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::services::runners;

/// Announced by a runner that can run a sandbox pod.
pub const SANDBOX_CAPABILITY: &str = "sandbox:pod";

#[derive(Debug)]
pub enum AgentRunError {
    Invalid {
        field: String,
        token: String,
    },
    NotFound,
    EnvironmentNotFound,
    /// The environment has no active build to run.
    NoActiveBuild,
    /// The run has already ended.
    Ended,
    Db(DbErr),
}

impl From<DbErr> for AgentRunError {
    fn from(e: DbErr) -> Self {
        Self::Db(e)
    }
}

/// Who a run token stands for, while its job runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RunPrincipal {
    pub id: Uuid,
    pub org_id: i64,
}

fn stmt(sql: &str, values: Vec<sea_orm::Value>) -> Statement {
    Statement::from_sql_and_values(DatabaseBackend::Postgres, sql, values)
}

fn hash(token: &str) -> String {
    format!("{:x}", Sha256::digest(token.as_bytes()))
}

fn invalid(field: &str, token: &str) -> AgentRunError {
    AgentRunError::Invalid {
        field: field.into(),
        token: token.into(),
    }
}

const ISO: &str = "'YYYY-MM-DD\"T\"HH24:MI:SS.MS\"Z\"'";

fn select() -> String {
    format!(
        "SELECT a.id, a.environment_id, a.job_id, a.llm_calls, a.llm_calls_max, \
           a.input_tokens, a.output_tokens, j.state, j.outcome, j.exit_code, \
           j.spec->>'image' AS image, \
           to_char(a.created_at AT TIME ZONE 'UTC', {ISO}) AS created_at, \
           to_char(j.started_at AT TIME ZONE 'UTC', {ISO}) AS started_at, \
           to_char(j.finished_at AT TIME ZONE 'UTC', {ISO}) AS finished_at \
         FROM agent_runs a JOIN runner_jobs j ON j.id = a.job_id"
    )
}

fn view(r: &QueryResult) -> Result<AgentRun, DbErr> {
    let state: String = r.try_get("", "state")?;
    let outcome: Option<String> = r.try_get("", "outcome")?;
    Ok(AgentRun {
        id: r.try_get::<Uuid>("", "id")?.to_string(),
        environment_id: r.try_get::<Uuid>("", "environment_id")?.to_string(),
        image: r.try_get("", "image")?,
        job_id: r.try_get::<Uuid>("", "job_id")?.to_string(),
        state: RunnerJobState::parse(&state)
            .ok_or_else(|| DbErr::Custom(format!("unknown job state {state}")))?,
        outcome: outcome.as_deref().and_then(RunnerJobOutcome::parse),
        exit_code: r.try_get("", "exit_code")?,
        llm_calls: r.try_get("", "llm_calls")?,
        llm_calls_max: r.try_get("", "llm_calls_max")?,
        input_tokens: r.try_get("", "input_tokens")?,
        output_tokens: r.try_get("", "output_tokens")?,
        created_at: r.try_get("", "created_at")?,
        started_at: r.try_get("", "started_at")?,
        finished_at: r.try_get("", "finished_at")?,
    })
}

/// Queues a run on the active build of an environment of the org.
pub async fn create(
    db: &DatabaseConnection,
    org_id: i64,
    user_id: i64,
    body: &AgentRunCreate,
) -> Result<AgentRun, AgentRunError> {
    if let Some((field, token)) = create_error(body) {
        return Err(invalid(field, token));
    }
    let environment =
        Uuid::parse_str(&body.environment_id).map_err(|_| AgentRunError::EnvironmentNotFound)?;
    let row = db
        .query_one_raw(stmt(
            "SELECT e.active_build_id, b.repository, b.digest FROM environments e \
             LEFT JOIN environment_builds b ON b.id = e.active_build_id \
             WHERE e.org_id = $1 AND e.id = $2",
            vec![org_id.into(), environment.into()],
        ))
        .await?
        .ok_or(AgentRunError::EnvironmentNotFound)?;
    let build: Option<Uuid> = row.try_get("", "active_build_id")?;
    let digest: Option<String> = row.try_get("", "digest")?;
    let (Some(build), Some(digest)) = (build, digest) else {
        return Err(AgentRunError::NoActiveBuild);
    };
    let repository: String = row.try_get("", "repository")?;
    let spec = RunnerJobSpec {
        image: format!("{repository}@{digest}"),
        command: body.command.clone(),
        timeout_secs: body.timeout_secs,
    };
    if spec_error(&spec).is_some() {
        return Err(AgentRunError::NoActiveBuild);
    }
    let txn = db.begin().await?;
    let job = runners::create_job(
        &txn,
        org_id,
        user_id,
        &RunnerJobCreate {
            kind: RunnerJobKind::Sandbox,
            spec,
            required_capabilities: vec![SANDBOX_CAPABILITY.to_string()],
        },
    )
    .await?;
    let id = Uuid::new_v4();
    let job_id = Uuid::parse_str(&job.id).map_err(|e| DbErr::Custom(e.to_string()))?;
    txn.execute_raw(stmt(
        "INSERT INTO agent_runs \
           (id, org_id, environment_id, build_id, job_id, env, llm_calls_max, created_by) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8)",
        vec![
            id.into(),
            org_id.into(),
            environment.into(),
            build.into(),
            job_id.into(),
            serde_json::json!(body.env).into(),
            body.llm_calls_max
                .unwrap_or(pnex_core::agent_runs::LLM_CALLS_MAX_DEFAULT)
                .into(),
            user_id.into(),
        ],
    ))
    .await?;
    txn.commit().await?;
    get(db, org_id, id).await
}

pub async fn get(
    db: &DatabaseConnection,
    org_id: i64,
    id: Uuid,
) -> Result<AgentRun, AgentRunError> {
    let row = db
        .query_one_raw(stmt(
            &format!("{} WHERE a.org_id = $1 AND a.id = $2", select()),
            vec![org_id.into(), id.into()],
        ))
        .await?
        .ok_or(AgentRunError::NotFound)?;
    Ok(view(&row)?)
}

pub async fn list(
    db: &DatabaseConnection,
    org_id: i64,
    limit: i64,
    offset: i64,
) -> Result<(i64, Vec<AgentRun>), DbErr> {
    let total: i64 = db
        .query_one_raw(stmt(
            "SELECT count(*) AS n FROM agent_runs WHERE org_id = $1",
            vec![org_id.into()],
        ))
        .await?
        .map(|r| r.try_get("", "n"))
        .transpose()?
        .unwrap_or(0);
    let rows = db
        .query_all_raw(stmt(
            &format!(
                "{} WHERE a.org_id = $1 ORDER BY a.created_at DESC LIMIT $2 OFFSET $3",
                select()
            ),
            vec![org_id.into(), limit.into(), offset.into()],
        ))
        .await?;
    Ok((total, rows.iter().map(view).collect::<Result<_, _>>()?))
}

/// Ends a run now. Its token stops working at once; the runner learns it at its next
/// heartbeat and destroys the pod.
pub async fn stop(
    db: &DatabaseConnection,
    org_id: i64,
    id: Uuid,
) -> Result<AgentRun, AgentRunError> {
    let run = get(db, org_id, id).await?;
    let done = db
        .execute_raw(stmt(
            "UPDATE runner_jobs SET state = 'failed', outcome = $3, finished_at = now(), \
               lease_expires_at = NULL \
             WHERE org_id = $1 AND id = (SELECT job_id FROM agent_runs WHERE org_id = $1 AND id = $2) \
               AND state IN ('queued', 'running')",
            vec![
                org_id.into(),
                id.into(),
                RunnerJobOutcome::Stopped.as_str().into(),
            ],
        ))
        .await?;
    if done.rows_affected() == 0 {
        return Err(
            if run.finished_at.is_some() || run.state == RunnerJobState::Expired {
                AgentRunError::Ended
            } else {
                AgentRunError::NotFound
            },
        );
    }
    get(db, org_id, id).await
}

/// Where sandboxes reach the gateway: `PNEX_AGENT_GATEWAY_URL`, else the production host.
pub fn gateway_url() -> Option<String> {
    std::env::var("PNEX_AGENT_GATEWAY_URL")
        .ok()
        .map(|u| u.trim().trim_end_matches('/').to_string())
        .filter(|u| !u.is_empty())
        .or_else(|| crate::app::prod_host().map(|h| format!("https://{h}")))
}

/// The gateway's addresses, for the egress rule of the sandbox. A NetworkPolicy knows
/// addresses only (Q-E13): they are read now, at the claim.
async fn gateway_egress(url: &str) -> Vec<SandboxEgress> {
    let Ok(parsed) = reqwest::Url::parse(url) else {
        return Vec::new();
    };
    let (Some(host), Some(port)) = (parsed.host_str(), parsed.port_or_known_default()) else {
        return Vec::new();
    };
    let Ok(found) = tokio::net::lookup_host((host, port)).await else {
        return Vec::new();
    };
    let mut out: Vec<SandboxEgress> = found
        .map(|a| SandboxEgress {
            cidr: match a.ip() {
                std::net::IpAddr::V4(ip) => format!("{ip}/32"),
                std::net::IpAddr::V6(ip) => format!("{ip}/128"),
            },
            port,
        })
        .collect();
    out.sort_by(|a, b| a.cidr.cmp(&b.cidr));
    out.dedup();
    out
}

/// What the sandbox of a claimed run needs, with a fresh run token: a token handed to a
/// runner that lost the job is dead from here on. `None`: no such run, or no gateway on
/// this deployment; the caller fails the job.
pub async fn sandbox_input(
    db: &DatabaseConnection,
    org_id: i64,
    job_id: &str,
) -> Option<RunnerSandboxInput> {
    let job = Uuid::parse_str(job_id).ok()?;
    let gateway = gateway_url()?;
    let mut bytes = [0u8; 32];
    rand::rng().fill(&mut bytes);
    let token = format!(
        "{RUN_TOKEN_PREFIX}{}",
        bytes.iter().map(|b| format!("{b:02x}")).collect::<String>()
    );
    let row = db
        .query_one_raw(stmt(
            "UPDATE agent_runs SET token_hash = $3 WHERE org_id = $1 AND job_id = $2 RETURNING env",
            vec![org_id.into(), job.into(), hash(&token).into()],
        ))
        .await
        .ok()??;
    let mut env: BTreeMap<String, String> =
        serde_json::from_value(row.try_get::<serde_json::Value>("", "env").ok()?).ok()?;
    let base = format!("{gateway}/api/v1/agent-gateway");
    env.insert("PNEX_GATEWAY_URL".into(), base.clone());
    env.insert("ANTHROPIC_BASE_URL".into(), base.clone());
    env.insert("OPENAI_BASE_URL".into(), format!("{base}/v1"));
    // Engines read their key from these: the run token stands in for it (D385).
    let secrets = BTreeMap::from([
        ("PNEX_RUN_TOKEN".to_string(), token.clone()),
        ("ANTHROPIC_API_KEY".to_string(), token.clone()),
        ("OPENAI_API_KEY".to_string(), token),
    ]);
    Some(RunnerSandboxInput {
        init_image: std::env::var("PNEX_SANDBOX_INIT_IMAGE").unwrap_or_default(),
        repos: Vec::new(),
        services: Vec::new(),
        setup: Vec::new(),
        env,
        secrets,
        egress: gateway_egress(&gateway).await,
    })
}

/// The run behind a token, only while its job runs.
pub async fn authenticate(
    db: &DatabaseConnection,
    token: &str,
) -> Result<Option<RunPrincipal>, DbErr> {
    if !token.starts_with(RUN_TOKEN_PREFIX) {
        return Ok(None);
    }
    let Some(r) = db
        .query_one_raw(stmt(
            "SELECT a.id, a.org_id FROM agent_runs a JOIN runner_jobs j ON j.id = a.job_id \
             WHERE a.token_hash = $1 AND j.state = 'running'",
            vec![hash(token).into()],
        ))
        .await?
    else {
        return Ok(None);
    };
    Ok(Some(RunPrincipal {
        id: r.try_get("", "id")?,
        org_id: r.try_get("", "org_id")?,
    }))
}

/// Counts one gateway call against the run's budget; `false` once it is spent.
pub async fn take_call(db: &DatabaseConnection, run: RunPrincipal) -> Result<bool, DbErr> {
    let done = db
        .execute_raw(stmt(
            "UPDATE agent_runs SET llm_calls = llm_calls + 1 \
             WHERE id = $1 AND org_id = $2 AND llm_calls < llm_calls_max",
            vec![run.id.into(), run.org_id.into()],
        ))
        .await?;
    Ok(done.rows_affected() == 1)
}

pub async fn add_tokens(
    db: &DatabaseConnection,
    run: RunPrincipal,
    usage: pnex_core::agent_runs::LlmUsageCount,
) -> Result<(), DbErr> {
    db.execute_raw(stmt(
        "UPDATE agent_runs SET input_tokens = input_tokens + $3, \
           output_tokens = output_tokens + $4 WHERE id = $1 AND org_id = $2",
        vec![
            run.id.into(),
            run.org_id.into(),
            usage.input.into(),
            usage.output.into(),
        ],
    ))
    .await?;
    Ok(())
}
