//! Chains of agent runs (agent-environments.md D713, D716, D718, D765–D768). A chain has one
//! budget (runs, rounds of one agent, model calls of all its runs) and holds at most one
//! subject among the active chains of the org. It runs one run at a time. When a run ends,
//! its summary is published once on the org channel for the `agent_run_finished` nodes.

use std::collections::BTreeMap;
use std::time::Duration;

use pnex_core::agent_chains::{
    finished_channel, AgentChain, AgentRunFinishedEvent, ChainLimits, ChainState,
    EXHAUSTED_LLM_CALLS, EXHAUSTED_ROUNDS, EXHAUSTED_RUNS,
};
use sea_orm::{
    ConnectionTrait, DatabaseBackend, DatabaseConnection, DbErr, QueryResult, Statement,
};
use uuid::Uuid;

use crate::services::agent_runs::AgentRunError;

const VALKEY_TIMEOUT: Duration = Duration::from_secs(2);
const ISO: &str = "'YYYY-MM-DD\"T\"HH24:MI:SS.MS\"Z\"'";

fn stmt(sql: &str, values: Vec<sea_orm::Value>) -> Statement {
    Statement::from_sql_and_values(DatabaseBackend::Postgres, sql, values)
}

/// Which chain a new run goes to.
#[derive(Debug, Clone)]
pub enum ChainPick {
    Join(Uuid),
    Open {
        subject: Option<String>,
        limits: ChainLimits,
    },
}

/// Takes a place in a chain for a run of `agent`, inside the transaction that creates the
/// run: the chain row stays locked until it commits. Returns the chain and the model calls
/// the run may make, never above what the chain has left.
pub async fn take<C: ConnectionTrait>(
    txn: &C,
    org_id: i64,
    user_id: i64,
    agent: Option<Uuid>,
    pick: &ChainPick,
    llm_calls_max: i32,
) -> Result<(Uuid, i32), AgentRunError> {
    match pick {
        ChainPick::Open { subject, limits } => {
            let id = Uuid::new_v4();
            let row = txn
                .query_one_raw(stmt(
                    "INSERT INTO agent_chains (id, org_id, subject, runs_max, llm_calls_max, rounds_max, created_by) \
                     VALUES ($1, $2, $3, $4, $5, $6, $7) \
                     ON CONFLICT (org_id, subject) WHERE state = 'active' AND subject IS NOT NULL DO NOTHING \
                     RETURNING id",
                    vec![
                        id.into(),
                        org_id.into(),
                        subject.clone().into(),
                        limits.runs_max.into(),
                        limits.llm_calls_max.into(),
                        limits.rounds_max.into(),
                        user_id.into(),
                    ],
                ))
                .await?;
            if row.is_some() {
                return Ok((id, llm_calls_max.min(limits.llm_calls_max)));
            }
            // The subject has its chain already: a repeated report joins it (D718).
            let held = txn
                .query_one_raw(stmt(
                    "SELECT id FROM agent_chains WHERE org_id = $1 AND subject = $2 AND state = 'active'",
                    vec![org_id.into(), subject.clone().into()],
                ))
                .await?
                .ok_or(AgentRunError::ChainBusy)?;
            Err(AgentRunError::SubjectTaken {
                chain_id: held.try_get("", "id")?,
            })
        }
        ChainPick::Join(id) => {
            let chain = txn
                .query_one_raw(stmt(
                    "SELECT state, runs_max, llm_calls_max, rounds_max FROM agent_chains \
                     WHERE org_id = $1 AND id = $2 FOR UPDATE",
                    vec![org_id.into(), (*id).into()],
                ))
                .await?
                .ok_or(AgentRunError::ChainNotFound)?;
            if chain.try_get::<String>("", "state")? != ChainState::Active.as_str() {
                return Err(AgentRunError::ChainClosed);
            }
            let used = txn
                .query_one_raw(stmt(
                    "SELECT count(*) AS runs, coalesce(sum(a.llm_calls), 0)::bigint AS llm_calls, \
                       count(*) FILTER (WHERE a.agent_id = $2) AS rounds, \
                       count(*) FILTER (WHERE j.state IN ('queued', 'running')) AS busy \
                     FROM agent_runs a JOIN runner_jobs j ON j.id = a.job_id WHERE a.chain_id = $1",
                    vec![(*id).into(), agent.into()],
                ))
                .await?
                .ok_or(AgentRunError::ChainNotFound)?;
            let n = |k: &str| used.try_get::<i64>("", k);
            if n("busy")? > 0 {
                return Err(AgentRunError::ChainBusy);
            }
            let max = |k: &str| chain.try_get::<i32>("", k).map(i64::from);
            let exhausted = |reason: &'static str| AgentRunError::ChainExhausted { reason };
            if n("runs")? >= max("runs_max")? {
                return Err(exhausted(EXHAUSTED_RUNS));
            }
            if agent.is_some() && n("rounds")? >= max("rounds_max")? {
                return Err(exhausted(EXHAUSTED_ROUNDS));
            }
            // No run of the chain is running: what the ended ones used is final.
            let left = max("llm_calls_max")? - n("llm_calls")?;
            if left <= 0 {
                return Err(exhausted(EXHAUSTED_LLM_CALLS));
            }
            Ok((*id, i64::from(llm_calls_max).min(left) as i32))
        }
    }
}

fn view(r: &QueryResult) -> Result<AgentChain, DbErr> {
    let state: String = r.try_get("", "state")?;
    Ok(AgentChain {
        id: r.try_get::<Uuid>("", "id")?.to_string(),
        subject: r.try_get("", "subject")?,
        state: ChainState::parse(&state)
            .ok_or_else(|| DbErr::Custom(format!("unknown chain state {state}")))?,
        limits: ChainLimits {
            runs_max: r.try_get("", "runs_max")?,
            llm_calls_max: r.try_get("", "llm_calls_max")?,
            rounds_max: r.try_get("", "rounds_max")?,
        },
        runs: r.try_get("", "runs")?,
        llm_calls: r.try_get("", "llm_calls")?,
        created_at: r.try_get("", "created_at")?,
        closed_at: r.try_get("", "closed_at")?,
    })
}

pub async fn get(
    db: &DatabaseConnection,
    org_id: i64,
    id: Uuid,
) -> Result<AgentChain, AgentRunError> {
    let row = db
        .query_one_raw(stmt(
            &format!(
                "SELECT c.id, c.subject, c.state, c.runs_max, c.llm_calls_max, c.rounds_max, \
                   (SELECT count(*) FROM agent_runs a WHERE a.chain_id = c.id) AS runs, \
                   (SELECT coalesce(sum(a.llm_calls), 0)::bigint FROM agent_runs a WHERE a.chain_id = c.id) AS llm_calls, \
                   to_char(c.created_at AT TIME ZONE 'UTC', {ISO}) AS created_at, \
                   to_char(c.closed_at AT TIME ZONE 'UTC', {ISO}) AS closed_at \
                 FROM agent_chains c WHERE c.org_id = $1 AND c.id = $2"
            ),
            vec![org_id.into(), id.into()],
        ))
        .await?
        .ok_or(AgentRunError::ChainNotFound)?;
    Ok(view(&row)?)
}

/// A person's gesture: the chain takes no more runs and its subject is free (D718). A run
/// still running goes on; stopping it is its own gesture.
pub async fn close(
    db: &DatabaseConnection,
    org_id: i64,
    id: Uuid,
) -> Result<AgentChain, AgentRunError> {
    let chain = get(db, org_id, id).await?;
    if chain.state == ChainState::Closed {
        return Err(AgentRunError::ChainClosed);
    }
    db.execute_raw(stmt(
        "UPDATE agent_chains SET state = 'closed', closed_at = now() \
         WHERE org_id = $1 AND id = $2 AND state = 'active'",
        vec![org_id.into(), id.into()],
    ))
    .await?;
    get(db, org_id, id).await
}

/// The summary of the run behind ended job `job`, claimed once: `None` when it has no run,
/// has not ended, or was already published (D398 anti-storm).
pub async fn claim_finished<C: ConnectionTrait>(
    db: &C,
    org_id: i64,
    job: Uuid,
) -> Result<Option<AgentRunFinishedEvent>, DbErr> {
    let Some(r) = db
        .query_one_raw(stmt(
            "UPDATE agent_runs a SET finished_event_at = now() FROM runner_jobs j \
             WHERE j.id = a.job_id AND a.org_id = $1 AND a.job_id = $2 \
               AND a.finished_event_at IS NULL AND j.state NOT IN ('queued', 'running') \
             RETURNING a.id, a.agent_id, a.chain_id, a.llm_calls, a.error, j.state, j.outcome, j.exit_code",
            vec![org_id.into(), job.into()],
        ))
        .await?
    else {
        return Ok(None);
    };
    let run: Uuid = r.try_get("", "id")?;
    let chain: Option<Uuid> = r.try_get("", "chain_id")?;
    let subject = match chain {
        Some(c) => db
            .query_one_raw(stmt(
                "SELECT subject FROM agent_chains WHERE id = $1",
                vec![c.into()],
            ))
            .await?
            .map(|s| s.try_get::<Option<String>>("", "subject"))
            .transpose()?
            .flatten(),
        None => None,
    };
    let outputs: BTreeMap<String, String> = db
        .query_all_raw(stmt(
            "SELECT id, name FROM agent_artifacts WHERE org_id = $1 AND run_id = $2 AND state = 'ready'",
            vec![org_id.into(), run.into()],
        ))
        .await?
        .iter()
        .map(|o| Ok((o.try_get("", "name")?, o.try_get::<Uuid>("", "id")?.to_string())))
        .collect::<Result<_, DbErr>>()?;
    Ok(Some(AgentRunFinishedEvent {
        run_id: run.to_string(),
        agent_id: r
            .try_get::<Option<Uuid>>("", "agent_id")?
            .map(|u| u.to_string()),
        chain_id: chain.map(|u| u.to_string()),
        subject,
        state: r.try_get("", "state")?,
        outcome: r.try_get("", "outcome")?,
        exit_code: r.try_get("", "exit_code")?,
        error: r.try_get("", "error")?,
        llm_calls: r.try_get("", "llm_calls")?,
        outputs,
        ts_ms: chrono::Utc::now().timestamp_millis(),
    }))
}

/// Publishes the end of the run behind `job` to the deployed flows of the org.
// shortcut: claimed before the publish, so a Valkey outage loses that one event (no
// retry); add an outbox swept in the background if flows ever miss ends in practice.
pub async fn publish_finished(
    db: &DatabaseConnection,
    valkey: Option<redis::aio::ConnectionManager>,
    org_id: i64,
    job: Uuid,
) -> Result<(), DbErr> {
    let Some(event) = claim_finished(db, org_id, job).await? else {
        return Ok(());
    };
    let Some(mut conn) = valkey else {
        tracing::warn!(org_id, %job, "run end not published: no valkey");
        return Ok(());
    };
    let frame = serde_json::to_string(&event).map_err(|e| DbErr::Custom(e.to_string()))?;
    let op = redis::AsyncCommands::publish::<_, _, i64>(&mut conn, finished_channel(org_id), frame);
    match tokio::time::timeout(VALKEY_TIMEOUT, op).await {
        Ok(Ok(_)) => {}
        Ok(Err(e)) => tracing::warn!(org_id, %job, error = %e, "run end not published"),
        Err(_) => tracing::warn!(org_id, %job, "run end not published: valkey timeout"),
    }
    Ok(())
}

/// One message of a deployed `agent_run` node (D713). The runtime is untrusted: the agent,
/// the chain bounds and the deployer come from the database, the message gives values
/// only. The run acts within the rights of the person who deployed the flow, re-read now:
/// one who lost `agents:run` since then starts nothing (D757, R2).
pub async fn run_node(
    db: &DatabaseConnection,
    artifacts: crate::services::agent_artifacts::ArtifactSettings,
    req: &pnex_core::agent_chains::AgentRunNodeRequest,
) -> Result<
    pnex_core::agent_chains::AgentRunNodeReply,
    crate::services::ontology::flow_nodes::NodeError,
> {
    use crate::services::agents::AgentError;
    use crate::services::ontology::flow_nodes::{deployed, NodeError};
    use pnex_core::agent_chains::{AgentRunNodeReply, NODE_PORT_REFUSED, NODE_PORT_STARTED};
    use pnex_core::err_codes;
    let pnex_core::FlowNodeKind::AgentRun { config } = deployed(db, &req.node).await? else {
        return Err(NodeError::NotDeployed);
    };
    let refused = |code: &str, args: serde_json::Value| AgentRunNodeReply {
        port: NODE_PORT_REFUSED,
        code: Some(code.to_string()),
        args,
        ..Default::default()
    };
    let org_id = req.node.org_id;
    let deployer: Option<i64> = db
        .query_one_raw(stmt(
            "SELECT deployed_by FROM flows WHERE org_id = $1 AND id = $2",
            vec![org_id.into(), req.node.flow_id.into()],
        ))
        .await?
        .map(|r| r.try_get("", "deployed_by"))
        .transpose()?
        .flatten();
    let grants = match deployer {
        Some(user) => crate::services::iam::store::member_grants(db, org_id, user).await?,
        None => None,
    };
    let (Some(user), Some(grants)) = (deployer, grants) else {
        return Ok(refused(err_codes::ACCESS_DENIED, serde_json::json!({})));
    };
    if !grants.can("agents:run") {
        return Ok(refused(
            err_codes::ACCESS_DENIED,
            serde_json::json!({ "permission": "agents:run" }),
        ));
    }
    let Ok(agent) = Uuid::parse_str(&config.agent_id) else {
        return Err(NodeError::NotDeployed);
    };
    crate::services::ontology::flow_nodes::spend(org_id)?;
    // Only the inputs the deployed node maps; the run checks they are ready artifacts.
    let inputs = config
        .inputs
        .keys()
        .filter_map(|name| {
            req.inputs
                .get(name)
                .map(|id| pnex_core::agent_runs::ArtifactInput {
                    name: name.clone(),
                    artifact_id: id.clone(),
                })
        })
        .collect();
    let joins = req.chain_id.is_some();
    let start = pnex_core::agents::AgentRunStart {
        inputs,
        chain_id: req.chain_id.clone(),
        subject: if joins { None } else { req.subject.clone() },
        chain_limits: if joins { None } else { Some(config.limits) },
        ..Default::default()
    };
    let started =
        crate::services::agents::start_run(db, artifacts, org_id, user, agent, &start).await;
    let (code, args) = match started {
        Ok(run) => {
            return Ok(AgentRunNodeReply {
                port: NODE_PORT_STARTED,
                run_id: Some(run.id),
                chain_id: run.chain_id,
                ..Default::default()
            })
        }
        Err(AgentError::Run(AgentRunError::SubjectTaken { chain_id })) => {
            return Ok(AgentRunNodeReply {
                port: NODE_PORT_STARTED,
                chain_id: Some(chain_id.to_string()),
                joined: true,
                ..Default::default()
            })
        }
        Err(AgentError::Invalid { field, token }) => (
            err_codes::FIELD_INVALID_BODY,
            serde_json::json!({ "field": field, "token": token }),
        ),
        Err(AgentError::Run(AgentRunError::Invalid { field, token })) => (
            err_codes::FIELD_INVALID_BODY,
            serde_json::json!({ "field": field, "token": token }),
        ),
        Err(AgentError::NotFound) => (err_codes::AGENT_NOT_FOUND, serde_json::json!({})),
        Err(AgentError::ProviderNotFound) => {
            (err_codes::AGENT_PROVIDER_NOT_FOUND, serde_json::json!({}))
        }
        Err(AgentError::Run(e)) => match e {
            AgentRunError::ChainNotFound => {
                (err_codes::AGENT_CHAIN_NOT_FOUND, serde_json::json!({}))
            }
            AgentRunError::ChainClosed => (err_codes::AGENT_CHAIN_CLOSED, serde_json::json!({})),
            AgentRunError::ChainExhausted { reason } => (
                err_codes::AGENT_CHAIN_EXHAUSTED,
                serde_json::json!({ "reason": reason }),
            ),
            AgentRunError::ChainBusy => (err_codes::AGENT_CHAIN_BUSY, serde_json::json!({})),
            AgentRunError::ArtifactNotFound => {
                (err_codes::AGENT_ARTIFACT_NOT_FOUND, serde_json::json!({}))
            }
            AgentRunError::EnvironmentNotFound => {
                (err_codes::ENVIRONMENT_NOT_FOUND, serde_json::json!({}))
            }
            AgentRunError::NoActiveBuild => {
                (err_codes::AGENT_RUN_NO_ACTIVE_BUILD, serde_json::json!({}))
            }
            AgentRunError::ScanRefused { breaches } => (
                err_codes::ENVIRONMENT_SCAN_REFUSED,
                serde_json::json!({ "breaches": breaches.to_string() }),
            ),
            AgentRunError::Db(e) => return Err(e.into()),
            other => {
                tracing::warn!(org_id, error = ?other, "agent_run node refused");
                (err_codes::AGENT_RUN_NOT_FOUND, serde_json::json!({}))
            }
        },
        Err(AgentError::Db(e)) => return Err(e.into()),
        Err(other) => {
            tracing::warn!(org_id, error = ?other, "agent_run node refused");
            (err_codes::AGENT_NOT_FOUND, serde_json::json!({}))
        }
    };
    Ok(refused(code, args))
}
