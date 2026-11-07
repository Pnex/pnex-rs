//! Rule runs played by a browser agent (agent-unification.md D781–D783):
//! the `extension` engine. A run started for it waits queued in
//! `runner_jobs`; the link of an agent allowed to play it (its creator's own
//! browser, else a common one, D550) leases it, one at a time, and receives
//! the sealed plan. The agent plays it to the end and sends back each step,
//! each extract and the outcome; the server checks every page they come
//! from against the plan (its site and allowed hosts) and the objections
//! (D341) before keeping them. The cadence and the caps live here, never in
//! the agent.

use pnex_core::site_recipe::{
    reason, site_base, site_host, RulePlan, RunExtract, RunStatus, RunStep, StepStatus,
};
use sea_orm::{ConnectionTrait, DatabaseBackend, DbErr, Statement};
use uuid::Uuid;

use crate::services::extension::AgentAuth;

/// Lease of a run, renewed by every message and every refresh of the link;
/// a silent agent loses it and the reaper gives the run back to the queue.
const LEASE: &str = "2 minutes";

/// Text kept per extract, like the server browser.
const EXTRACT_MAX: usize = 200_000;

fn stmt(sql: &str, values: Vec<sea_orm::Value>) -> Statement {
    Statement::from_sql_and_values(DatabaseBackend::Postgres, sql, values)
}

fn agent_key(agent: &AgentAuth) -> String {
    agent.device_registry_id.to_string()
}

/// Renews the runs the agent holds; when it holds none, leases the oldest
/// queued run it may play (fresh: what an earlier lease left is cleared).
/// Answers the plans it holds.
pub async fn held(db: &impl ConnectionTrait, agent: &AgentAuth) -> Result<Vec<RulePlan>, DbErr> {
    let renewed = db
        .execute_raw(stmt(
            &format!(
                "UPDATE runner_jobs SET lease_expires_at = now() + interval '{LEASE}'
                 WHERE org_id = $1 AND kind = 'extension' AND state = 'running'
                   AND spec ->> 'agent' = $2"
            ),
            vec![agent.org_id.into(), agent_key(agent).into()],
        ))
        .await?
        .rows_affected();
    if renewed == 0 {
        let leased = db
            .query_one_raw(stmt(
                &format!(
                    "WITH pick AS (
                       SELECT j.id FROM runner_jobs j
                       WHERE j.org_id = $1 AND j.kind = 'extension' AND j.state = 'queued'
                         AND j.queue_expires_at > now()
                         AND EXISTS (SELECT 1 FROM extension_agents a
                           WHERE a.device_registry_id = $2 AND (j.created_by = a.paired_by
                             OR (a.common AND NOT EXISTS (SELECT 1 FROM extension_agents own
                               WHERE own.org_id = j.org_id AND own.paired_by = j.created_by
                                 AND NOT own.common))))
                       ORDER BY j.created_at LIMIT 1 FOR UPDATE SKIP LOCKED)
                     UPDATE runner_jobs j SET state = 'running', started_at = now(),
                       attempts = j.attempts + 1, lease_expires_at = now() + interval '{LEASE}',
                       spec = jsonb_build_object('agent', $3::text)
                     FROM pick WHERE j.id = pick.id RETURNING j.id"
                ),
                vec![
                    agent.org_id.into(),
                    agent.device_registry_id.into(),
                    agent_key(agent).into(),
                ],
            ))
            .await?
            .and_then(|r| r.try_get::<Uuid>("", "id").ok());
        if let Some(job) = leased {
            db.execute_raw(stmt(
                "UPDATE agent_runs SET steps = '[]', extracts = '[]', failed_step = NULL,
                   reason = NULL WHERE job_id = $1",
                vec![job.into()],
            ))
            .await?;
        }
    }
    let rows = db
        .query_all_raw(stmt(
            "SELECT a.id, a.plan FROM agent_runs a JOIN runner_jobs j ON j.id = a.job_id
             WHERE j.org_id = $1 AND j.kind = 'extension' AND j.state = 'running'
               AND j.spec ->> 'agent' = $2
             ORDER BY j.started_at",
            vec![agent.org_id.into(), agent_key(agent).into()],
        ))
        .await?;
    let mut plans = Vec::new();
    for r in &rows {
        let (Ok(id), Ok(plan)) = (
            r.try_get::<Uuid>("", "id"),
            r.try_get::<serde_json::Value>("", "plan"),
        ) else {
            continue;
        };
        if let Some(p) = plan_of(id, &plan) {
            plans.push(p);
        }
    }
    Ok(plans)
}

/// The sealed plan of a run: its rule parsed again, its checked values.
fn plan_of(id: Uuid, plan: &serde_json::Value) -> Option<RulePlan> {
    let recipe = super::parse(plan["yaml"].as_str()?).ok()?;
    let params = serde_json::from_value(plan["params"].clone()).unwrap_or_default();
    Some(RulePlan {
        run_id: id.to_string(),
        base: site_base(&recipe.site)?,
        site: site_host(&recipe.site)?,
        recipe,
        params,
    })
}

/// The plan of `run_id` when this agent holds it.
async fn own(
    db: &impl ConnectionTrait,
    agent: &AgentAuth,
    run_id: &str,
) -> Result<Option<RulePlan>, DbErr> {
    let Ok(id) = Uuid::parse_str(run_id) else {
        return Ok(None);
    };
    let row = db
        .query_one_raw(stmt(
            &format!(
                "WITH j AS (UPDATE runner_jobs SET lease_expires_at = now() + interval '{LEASE}'
                   WHERE id = $1 AND org_id = $2 AND kind = 'extension' AND state = 'running'
                     AND spec ->> 'agent' = $3 RETURNING id)
                 SELECT a.plan FROM agent_runs a JOIN j ON j.id = a.job_id"
            ),
            vec![id.into(), agent.org_id.into(), agent_key(agent).into()],
        ))
        .await?;
    Ok(row
        .and_then(|r| r.try_get::<serde_json::Value>("", "plan").ok())
        .and_then(|p| plan_of(id, &p)))
}

/// A page of the plan: its site, an allowed host, or no page yet.
fn on_plan(plan: &RulePlan, url: &str) -> bool {
    if url.is_empty() || url.starts_with("about:") {
        return true;
    }
    match site_host(url) {
        Some(h) => {
            h == plan.site
                || plan
                    .recipe
                    .allow_hosts
                    .iter()
                    .any(|a| site_host(a).as_deref() == Some(h.as_str()))
        }
        None => false,
    }
}

/// An address that objected (D341): a profile on the opposition list.
async fn opposed(db: &impl ConnectionTrait, org_id: i64, url: &str) -> Result<bool, DbErr> {
    let Ok((_, canonical)) = pnex_core::browser_agent::profile_of(url) else {
        return Ok(false);
    };
    Ok(db
        .query_one_raw(stmt(
            "SELECT 1 AS x FROM prospecting_oppositions WHERE org_id = $1 AND fingerprint = $2",
            vec![org_id.into(), canonical.into()],
        ))
        .await?
        .is_some())
}

/// Ends the run as failed at `step` for `why`.
async fn fail(
    ctx: &loco_rs::app::AppContext,
    agent: &AgentAuth,
    run_id: Uuid,
    step: Option<u32>,
    why: &str,
) -> Result<(), DbErr> {
    super::finish(&ctx.db, run_id, RunStatus::Failed, step, Some(why)).await?;
    super::journal(ctx, agent.org_id, run_id).await;
    super::purpose::on_finished(ctx, agent.org_id, run_id).await;
    Ok(())
}

/// One played step. A step on a page outside the plan is kept as the
/// failure it is and ends the run (D783).
pub async fn step(
    ctx: &loco_rs::app::AppContext,
    agent: &AgentAuth,
    run_id: &str,
    mut step: RunStep,
) -> Result<(), DbErr> {
    let db = &ctx.db;
    let Some(plan) = own(db, agent, run_id).await? else {
        return Ok(());
    };
    let id = Uuid::parse_str(&plan.run_id).unwrap_or_default();
    // Screenshots stay with the server browser: none is stored for an agent.
    step.screenshot = None;
    let off = !on_plan(&plan, &step.url);
    if off {
        step.status = StepStatus::Failed;
        step.detail = Some(step.url.clone());
        step.reason = Some(reason::OFF_SITE.into());
    } else if step
        .reason
        .as_deref()
        .is_some_and(|r| !reason::ALL.contains(&r))
    {
        step.reason = Some(reason::BROWSER.into());
    }
    db.execute_raw(stmt(
        "UPDATE agent_runs SET steps = steps || jsonb_build_array($2::jsonb) WHERE id = $1",
        vec![
            id.into(),
            serde_json::to_value(&step).unwrap_or_default().into(),
        ],
    ))
    .await?;
    if off {
        tracing::info!(run = %id, url = %step.url, "agent run left its plan");
        fail(ctx, agent, id, Some(step.index), reason::OFF_SITE).await?;
    }
    Ok(())
}

/// One extract: kept unless it comes from outside the plan or from a
/// profile that objected; a later one of the same name replaces it.
pub async fn extract(
    ctx: &loco_rs::app::AppContext,
    agent: &AgentAuth,
    run_id: &str,
    mut e: RunExtract,
) -> Result<(), DbErr> {
    let db = &ctx.db;
    let Some(plan) = own(db, agent, run_id).await? else {
        return Ok(());
    };
    let id = Uuid::parse_str(&plan.run_id).unwrap_or_default();
    let from = e.from.clone().unwrap_or_default();
    if !on_plan(&plan, &from) {
        tracing::info!(run = %id, url = %from, "agent extract off the plan, dropped");
        return Ok(());
    }
    if opposed(db, agent.org_id, &from).await? {
        tracing::info!(run = %id, "agent extract of an objecting profile, dropped");
        return Ok(());
    }
    // An agent stores no file in the run: its PDFs come as text only.
    e.file = None;
    if e.text.chars().count() > EXTRACT_MAX {
        e.text = e.text.chars().take(EXTRACT_MAX).collect();
    }
    db.execute_raw(stmt(
        "UPDATE agent_runs SET extracts = (
           SELECT coalesce(jsonb_agg(x), '[]'::jsonb) FROM jsonb_array_elements(extracts) x
           WHERE x ->> 'name' <> $2) || jsonb_build_array($3::jsonb)
         WHERE id = $1",
        vec![
            id.into(),
            e.name.clone().into(),
            serde_json::to_value(&e).unwrap_or_default().into(),
        ],
    ))
    .await?;
    Ok(())
}

/// The end of a run; its journal lines are written like any engine's.
pub async fn done(
    ctx: &loco_rs::app::AppContext,
    agent: &AgentAuth,
    run_id: &str,
    ok: bool,
    failed_step: Option<u32>,
    why: Option<&str>,
) -> Result<(), DbErr> {
    let Some(plan) = own(&ctx.db, agent, run_id).await? else {
        return Ok(());
    };
    let id = Uuid::parse_str(&plan.run_id).unwrap_or_default();
    if ok {
        super::finish(&ctx.db, id, RunStatus::Ok, None, None).await?;
        super::journal(ctx, agent.org_id, id).await;
        super::purpose::on_finished(ctx, agent.org_id, id).await;
        return Ok(());
    }
    let why = why
        .filter(|r| reason::ALL.contains(r))
        .unwrap_or(reason::BROWSER);
    fail(ctx, agent, id, failed_step, why).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_page_is_on_the_plan_by_its_site_or_an_allowed_host() {
        let recipe = super::super::parse(
            "name: r\nsite: www.example.org\nallow_hosts: [cdn.example.net]\nsteps:\n  - open: \"/\"\n",
        )
        .unwrap();
        let plan = RulePlan {
            run_id: Uuid::nil().to_string(),
            base: "https://www.example.org".into(),
            site: "example.org".into(),
            recipe,
            params: Default::default(),
        };
        assert!(on_plan(&plan, "https://example.org/a"));
        assert!(on_plan(&plan, "https://cdn.example.net/x.pdf"));
        assert!(on_plan(&plan, ""));
        assert!(!on_plan(&plan, "https://evil.example/example.org"));
    }
}
