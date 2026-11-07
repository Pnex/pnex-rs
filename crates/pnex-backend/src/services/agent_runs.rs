//! Runs of hosted agents (agent-environments.md D731). A run is a `sandbox` job on the
//! active image of an environment. Its token is minted at each claim, stored hashed, and
//! opens the LLM gateway only while that job runs: a stop, the end of the job or a
//! takeover by another runner closes it at once.

use std::collections::BTreeMap;

use pnex_core::agent_runs::{
    create_error, service_env_name, AgentRun, AgentRunCreate, ArtifactInput, ArtifactOutput,
    RunRepo, RunService, GENERATED, RUN_TOKEN_PREFIX,
};
use pnex_core::runners::{
    spec_error, RunnerJobCreate, RunnerJobKind, RunnerJobOutcome, RunnerJobSpec, RunnerJobState,
    RunnerSandboxInput, SandboxArtifacts, SandboxEgress, SandboxRepo, SandboxService,
};
use rand::RngExt;
use sea_orm::{
    ConnectionTrait, DatabaseBackend, DatabaseConnection, DbErr, QueryResult, Statement,
    TransactionTrait,
};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::services::agent_artifacts::{self, ArtifactError, ArtifactSettings};
use crate::services::agent_chains::{self, ChainPick};
use crate::services::runners;
use crate::services::secrets::{store, Keyring};

/// Announced by a runner that can run a sandbox pod.

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
    /// An input names no ready artifact of the org.
    ArtifactNotFound,
    /// The active build is over the scan threshold and the org refuses such runs (D748).
    ScanRefused {
        breaches: u32,
    },
    /// No chain of the org has this id (D716).
    ChainNotFound,
    ChainClosed,
    /// The chain used its runs, rounds or model calls.
    ChainExhausted {
        reason: &'static str,
    },
    /// A run of the chain is still running (D718).
    ChainBusy,
    /// Another active chain holds the subject.
    SubjectTaken {
        chain_id: Uuid,
    },
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
    /// The provider of its agent, with its own key (D710); else the org's `agent` usage.
    pub provider_id: Option<Uuid>,
}

/// The agent a run is started from (D757).
#[derive(Debug, Clone, Copy)]
pub struct RunOrigin {
    pub agent_id: Uuid,
    pub agent_version: i32,
    pub provider_id: Option<Uuid>,
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

/// Hosted and internal agent runs, until the runs page shows every engine (D786, lot U5).
fn select() -> String {
    select_of("'sandbox', 'internal_llm'")
}

/// Every engine (D786): the runs page lists agents and rules alike.
const ALL_ENGINES: &str = "'sandbox', 'internal_llm', 'server_browser', 'extension'";

fn select_of(kinds: &str) -> String {
    format!(
        "SELECT a.id, a.environment_id, a.agent_id, a.agent_version, a.chain_id, a.job_id, a.llm_calls, a.llm_calls_max, \
           a.input_tokens, a.output_tokens, a.error, j.kind, j.state, j.outcome, j.exit_code, \
           COALESCE(j.spec->>'image', '') AS image, a.name, a.site, \
           to_char(a.created_at AT TIME ZONE 'UTC', {ISO}) AS created_at, \
           to_char(j.started_at AT TIME ZONE 'UTC', {ISO}) AS started_at, \
           to_char(j.finished_at AT TIME ZONE 'UTC', {ISO}) AS finished_at, \
           a.merge_requests \
         FROM agent_runs a JOIN runner_jobs j ON j.id = a.job_id \
           AND j.kind IN ({kinds})"
    )
}

fn view(r: &QueryResult) -> Result<AgentRun, DbErr> {
    let state: String = r.try_get("", "state")?;
    let outcome: Option<String> = r.try_get("", "outcome")?;
    let kind: String = r.try_get("", "kind")?;
    Ok(AgentRun {
        id: r.try_get::<Uuid>("", "id")?.to_string(),
        engine: RunnerJobKind::parse(&kind)
            .ok_or_else(|| DbErr::Custom(format!("unknown job kind {kind}")))?,
        environment_id: r
            .try_get::<Option<Uuid>>("", "environment_id")?
            .map(|u| u.to_string()),
        agent_id: r
            .try_get::<Option<Uuid>>("", "agent_id")?
            .map(|u| u.to_string()),
        agent_version: r.try_get("", "agent_version")?,
        chain_id: r
            .try_get::<Option<Uuid>>("", "chain_id")?
            .map(|u| u.to_string()),
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
        error: r.try_get("", "error")?,
        created_at: r.try_get("", "created_at")?,
        started_at: r.try_get("", "started_at")?,
        finished_at: r.try_get("", "finished_at")?,
        merge_requests: serde_json::from_value(r.try_get("", "merge_requests")?)
            .unwrap_or_default(),
        rule: matches!(kind.as_str(), "server_browser" | "extension")
            .then(|| r.try_get::<Option<String>>("", "name").ok().flatten())
            .flatten(),
        site: matches!(kind.as_str(), "server_browser" | "extension")
            .then(|| r.try_get::<Option<String>>("", "site").ok().flatten())
            .flatten(),
    })
}

/// Queues a run on the active build of an environment of the org.
pub async fn create(
    db: &DatabaseConnection,
    artifacts: ArtifactSettings,
    org_id: i64,
    user_id: i64,
    body: &AgentRunCreate,
) -> Result<AgentRun, AgentRunError> {
    create_as(db, artifacts, org_id, user_id, body, None, None).await
}

/// A run, bare or started from an agent whose record it carries.
pub async fn create_as(
    db: &DatabaseConnection,
    artifacts: ArtifactSettings,
    org_id: i64,
    user_id: i64,
    body: &AgentRunCreate,
    origin: Option<RunOrigin>,
    chain: Option<&ChainPick>,
) -> Result<AgentRun, AgentRunError> {
    if let Some((field, token)) = create_error(body) {
        return Err(invalid(field, token));
    }
    agent_artifacts::check_inputs(db, org_id, &body.inputs)
        .await
        .map_err(|e| match e {
            ArtifactError::Db(e) => AgentRunError::Db(e),
            _ => AgentRunError::ArtifactNotFound,
        })?;
    let environment =
        Uuid::parse_str(&body.environment_id).map_err(|_| AgentRunError::EnvironmentNotFound)?;
    let row = db
        .query_one_raw(stmt(
            "SELECT e.active_build_id, e.isolation, b.repository, b.digest FROM environments e \
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
    if let Some(breaches) =
        crate::services::environments::refused_by_scan(db, org_id, build).await?
    {
        return Err(AgentRunError::ScanRefused { breaches });
    }
    let repository: String = row.try_get("", "repository")?;
    let isolation =
        pnex_core::environments::Isolation::parse(&row.try_get::<String>("", "isolation")?)
            .unwrap_or_default();
    let spec = RunnerJobSpec {
        image: format!("{repository}@{digest}"),
        command: body.command.clone(),
        timeout_secs: body.timeout_secs,
    };
    if spec_error(&spec).is_some() {
        return Err(AgentRunError::NoActiveBuild);
    }
    let txn = db.begin().await?;
    let mut llm_calls_max = body
        .llm_calls_max
        .unwrap_or(pnex_core::agent_runs::LLM_CALLS_MAX_DEFAULT);
    let chain_id = match chain {
        Some(pick) => {
            let agent = origin.map(|o| o.agent_id);
            let (id, calls) =
                agent_chains::take(&txn, org_id, user_id, agent, pick, llm_calls_max).await?;
            llm_calls_max = calls;
            Some(id)
        }
        None => None,
    };
    let job = runners::create_job(
        &txn,
        org_id,
        user_id,
        &RunnerJobCreate {
            kind: RunnerJobKind::Sandbox,
            spec,
            required_capabilities: vec![isolation
                .capability(!body.services.is_empty())
                .to_string()],
        },
    )
    .await?;
    let id = Uuid::new_v4();
    let job_id = Uuid::parse_str(&job.id).map_err(|e| DbErr::Custom(e.to_string()))?;
    txn.execute_raw(stmt(
        "INSERT INTO agent_runs \
           (id, org_id, environment_id, build_id, job_id, env, llm_calls_max, created_by, \
            inputs, outputs, agent_id, agent_version, provider_id, repos, services, caches, chain_id) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16, $17)",
        vec![
            id.into(),
            org_id.into(),
            environment.into(),
            build.into(),
            job_id.into(),
            serde_json::json!(body.env).into(),
            llm_calls_max.into(),
            user_id.into(),
            serde_json::json!(body.inputs).into(),
            serde_json::json!(body.outputs).into(),
            origin.map(|o| o.agent_id).into(),
            origin.map(|o| o.agent_version).into(),
            origin.and_then(|o| o.provider_id).into(),
            serde_json::json!(body.repos).into(),
            serde_json::json!(body.services).into(),
            serde_json::json!(body.caches).into(),
            chain_id.into(),
        ],
    ))
    .await?;
    agent_artifacts::declare_outputs(&txn, artifacts, org_id, id, &body.outputs).await?;
    txn.commit().await?;
    get(db, org_id, id).await
}

/// A run of an internal agent (D785): an `internal_llm` job the backend plays, no
/// environment, no sandbox. Its final answer is the `report` output.
pub async fn create_internal(
    db: &DatabaseConnection,
    artifacts: ArtifactSettings,
    org_id: i64,
    user_id: i64,
    body: &AgentRunCreate,
    origin: RunOrigin,
    chain: Option<&ChainPick>,
) -> Result<AgentRun, AgentRunError> {
    agent_artifacts::check_inputs(db, org_id, &body.inputs)
        .await
        .map_err(|e| match e {
            ArtifactError::Db(e) => AgentRunError::Db(e),
            _ => AgentRunError::ArtifactNotFound,
        })?;
    let outputs = vec![ArtifactOutput {
        name: pnex_core::agents::REPORT_OUTPUT.into(),
        required: true,
    }];
    let txn = db.begin().await?;
    let mut llm_calls_max = body
        .llm_calls_max
        .unwrap_or(pnex_core::agent_runs::LLM_CALLS_MAX_DEFAULT);
    let chain_id = match chain {
        Some(pick) => {
            let (id, calls) = agent_chains::take(
                &txn,
                org_id,
                user_id,
                Some(origin.agent_id),
                pick,
                llm_calls_max,
            )
            .await?;
            llm_calls_max = calls;
            Some(id)
        }
        None => None,
    };
    let job = runners::create_job(
        &txn,
        org_id,
        user_id,
        &RunnerJobCreate {
            kind: RunnerJobKind::InternalLlm,
            // No image: the backend plays it; only the timeout is read.
            spec: RunnerJobSpec {
                image: String::new(),
                command: Vec::new(),
                timeout_secs: body.timeout_secs,
            },
            required_capabilities: Vec::new(),
        },
    )
    .await?;
    let id = Uuid::new_v4();
    let job_id = Uuid::parse_str(&job.id).map_err(|e| DbErr::Custom(e.to_string()))?;
    txn.execute_raw(stmt(
        "INSERT INTO agent_runs \
           (id, org_id, job_id, llm_calls_max, created_by, inputs, outputs, agent_id, \
            agent_version, provider_id, chain_id) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11)",
        vec![
            id.into(),
            org_id.into(),
            job_id.into(),
            llm_calls_max.into(),
            user_id.into(),
            serde_json::json!(body.inputs).into(),
            serde_json::json!(outputs).into(),
            origin.agent_id.into(),
            origin.agent_version.into(),
            origin.provider_id.into(),
            chain_id.into(),
        ],
    ))
    .await?;
    agent_artifacts::declare_outputs(&txn, artifacts, org_id, id, &outputs).await?;
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

/// The job of a run and when it was created, in microseconds: where its journal is.
pub async fn job_of(
    db: &DatabaseConnection,
    org_id: i64,
    id: Uuid,
) -> Result<(Uuid, i64), AgentRunError> {
    let row = db
        .query_one_raw(stmt(
            "SELECT job_id, (extract(epoch FROM created_at) * 1000000)::bigint AS at_us \
             FROM agent_runs WHERE org_id = $1 AND id = $2",
            vec![org_id.into(), id.into()],
        ))
        .await?
        .ok_or(AgentRunError::NotFound)?;
    Ok((row.try_get("", "job_id")?, row.try_get("", "at_us")?))
}

pub async fn list(
    db: &DatabaseConnection,
    org_id: i64,
    limit: i64,
    offset: i64,
    engine: Option<RunnerJobKind>,
) -> Result<(i64, Vec<AgentRun>), DbErr> {
    // Every engine, or the one asked (D786).
    let kinds = match engine {
        Some(e) => format!("'{}'", e.as_str()),
        None => ALL_ENGINES.to_string(),
    };
    let total: i64 = db
        .query_one_raw(stmt(
            &format!(
                "SELECT count(*) AS n FROM agent_runs a JOIN runner_jobs j ON j.id = a.job_id \
                 WHERE a.org_id = $1 AND j.kind IN ({kinds})"
            ),
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
                select_of(&kinds)
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
    // Nothing can be sent any more: its token died with the job.
    let job = Uuid::parse_str(&run.job_id).map_err(|e| DbErr::Custom(e.to_string()))?;
    agent_artifacts::close_outputs(db, org_id, job).await?;
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
    ring: Option<&Keyring>,
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
            "UPDATE agent_runs SET token_hash = $3 WHERE org_id = $1 AND job_id = $2 \
             RETURNING id, env, inputs, outputs, repos, services, caches",
            vec![org_id.into(), job.into(), hash(&token).into()],
        ))
        .await
        .ok()??;
    let inputs: Vec<ArtifactInput> =
        serde_json::from_value(row.try_get("", "inputs").ok()?).ok()?;
    let outputs: Vec<ArtifactOutput> =
        serde_json::from_value(row.try_get("", "outputs").ok()?).ok()?;
    let caches: Vec<pnex_core::agent_runs::RunCache> =
        serde_json::from_value(row.try_get("", "caches").ok()?).ok()?;
    // The skills pinned by its agent come as one more input (D711).
    let skills = crate::services::agent_skills::bundle_of(db, org_id, job)
        .await
        .ok()?;
    let nothing = inputs.is_empty() && outputs.is_empty() && caches.is_empty() && skills.is_none();
    let artifacts = if nothing {
        None
    } else {
        // Its own token, for the transfer containers only: the agent holds the run token.
        let transfer = agent_artifacts::new_token();
        db.execute_raw(stmt(
            "UPDATE agent_runs SET artifact_token_hash = $3 WHERE org_id = $1 AND job_id = $2",
            vec![
                org_id.into(),
                job.into(),
                agent_artifacts::token_hash(&transfer).into(),
            ],
        ))
        .await
        .ok()?;
        let (mut ins, outs) = agent_artifacts::sandbox_lists(db, org_id, &inputs, &outputs)
            .await
            .ok()?;
        ins.extend(
            skills
                .as_deref()
                .map(crate::services::agent_skills::sandbox_entry),
        );
        Some(SandboxArtifacts {
            url: format!("{gateway}/api/v1/agent-transfer"),
            token: transfer,
            inputs: ins,
            outputs: outs,
            caches: caches.clone(),
        })
    };
    let mut env: BTreeMap<String, String> =
        serde_json::from_value(row.try_get::<serde_json::Value>("", "env").ok()?).ok()?;
    let base = format!("{gateway}/api/v1/agent-gateway");
    env.insert("PNEX_GATEWAY_URL".into(), base.clone());
    env.insert("ANTHROPIC_BASE_URL".into(), base.clone());
    env.insert("OPENAI_BASE_URL".into(), format!("{base}/v1"));
    // Engines read their key from these: the run token stands in for it (D385).
    let mut secrets = BTreeMap::from([
        ("PNEX_RUN_TOKEN".to_string(), token.clone()),
        ("ANTHROPIC_API_KEY".to_string(), token.clone()),
        ("OPENAI_API_KEY".to_string(), token),
    ]);
    let repos: Vec<RunRepo> = serde_json::from_value(row.try_get("", "repos").ok()?).ok()?;
    let services: Vec<RunService> =
        serde_json::from_value(row.try_get("", "services").ok()?).ok()?;
    let mut sandbox_repos = Vec::new();
    for repo in &repos {
        // A token that cannot be read fails the run: a clone without it would only fail later.
        let token = match &repo.secret_id {
            Some(id) => {
                let id = Uuid::parse_str(id).ok()?;
                Some(store::reveal(db, ring?, Some(org_id), id).await.ok()?)
            }
            None => None,
        };
        // A branch output: the write token goes to the push sidecar only (D388).
        let push = match &repo.push_secret_id {
            Some(id) => {
                let id = Uuid::parse_str(id).ok()?;
                let run: Uuid = row.try_get("", "id").ok()?;
                Some(pnex_core::runners::SandboxPush {
                    branch: crate::services::forge::run_branch(&run.to_string()),
                    token: store::reveal(db, ring?, Some(org_id), id).await.ok()?,
                })
            }
            None => None,
        };
        sandbox_repos.push(SandboxRepo {
            url: repo.url.clone(),
            git_ref: repo.git_ref.clone(),
            path: repo.path.clone(),
            token,
            push,
        });
    }
    let mut sandbox_services = Vec::new();
    for service in &services {
        let mut env = service.env.clone();
        for (key, value) in env.iter_mut() {
            if value == GENERATED {
                *value = generated_password();
                // The agent connects with it: the same value, in its own secret.
                secrets.insert(service_env_name(&service.name, key), value.clone());
            }
        }
        sandbox_services.push(SandboxService {
            name: service.name.clone(),
            image: service.image.clone(),
            env,
            ready_command: service.ready_command.clone(),
            data_paths: service.data_paths.clone(),
            run_as_user: service.run_as_user,
        });
    }
    let mut egress = gateway_egress(&gateway).await;
    for repo in &repos {
        egress.extend(gateway_egress(&repo.url).await);
    }
    egress.sort_by(|a, b| a.cidr.cmp(&b.cidr));
    egress.dedup();
    Some(RunnerSandboxInput {
        init_image: std::env::var("PNEX_SANDBOX_INIT_IMAGE").unwrap_or_default(),
        repos: sandbox_repos,
        services: sandbox_services,
        setup: Vec::new(),
        env,
        secrets,
        egress,
        artifacts,
    })
}

/// Opens the merge request of each branch the run pushed (D388), once its job ended, and
/// keeps their URLs on the run. A refusal is logged as a code, never the forge's text.
pub async fn open_merge_requests(
    db: &DatabaseConnection,
    ring: Option<&Keyring>,
    org_id: i64,
    job: Uuid,
) -> Result<(), DbErr> {
    let Some(row) = db
        .query_one_raw(stmt(
            "SELECT id, repos FROM agent_runs WHERE org_id = $1 AND job_id = $2",
            vec![org_id.into(), job.into()],
        ))
        .await?
    else {
        return Ok(());
    };
    let run: Uuid = row.try_get("", "id")?;
    let repos: Vec<RunRepo> = serde_json::from_value(row.try_get("", "repos")?).unwrap_or_default();
    let mut urls = Vec::new();
    for repo in repos.iter().filter(|r| r.push_secret_id.is_some()) {
        let token = match (ring, repo.push_secret_id.as_deref().map(Uuid::parse_str)) {
            (Some(ring), Some(Ok(id))) => store::reveal(db, ring, Some(org_id), id).await.ok(),
            _ => None,
        };
        let Some(token) = token else {
            tracing::warn!(org_id, %run, "merge request not opened: write token unreadable");
            continue;
        };
        let branch = crate::services::forge::run_branch(&run.to_string());
        let body = format!(
            "Changes of PneX agent run `{run}`. Review them before merging: merging stays a human act."
        );
        match crate::services::forge::open_merge_request(
            &repo.url,
            &repo.git_ref,
            &branch,
            &token,
            &format!("Agent run {}", &run.to_string()[..8]),
            &body,
        )
        .await
        {
            Ok(Some(url)) => urls.push(url),
            Ok(None) => tracing::info!(org_id, %run, "no branch pushed, no merge request"),
            Err(code) => tracing::warn!(org_id, %run, code, "merge request not opened"),
        }
    }
    if !urls.is_empty() {
        db.execute_raw(stmt(
            "UPDATE agent_runs SET merge_requests = $3 WHERE org_id = $1 AND id = $2",
            vec![org_id.into(), run.into(), serde_json::json!(urls).into()],
        ))
        .await?;
    }
    Ok(())
}

/// A password for a test service, fresh at each claim (D717).
fn generated_password() -> String {
    let mut bytes = [0u8; 24];
    rand::rng().fill(&mut bytes);
    bytes.iter().map(|b| format!("{b:02x}")).collect()
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
            "SELECT a.id, a.org_id, a.provider_id FROM agent_runs a JOIN runner_jobs j ON j.id = a.job_id \
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
        provider_id: r.try_get("", "provider_id")?,
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
