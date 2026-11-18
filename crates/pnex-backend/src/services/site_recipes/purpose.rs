//! What the server does when a run of a default rule ends (agent-unification
//! D780, lot U4). A default rule is played like any rule; the purpose it was
//! started for is kept in its plan, and acting on the outcome is the server's
//! job: never a kind of mission, never the agent. Profile and employee list
//! captures are pipelines of the prospecting pack (pipeline-engine.md D996):
//! their navigations resume their rows ([`Purpose::Pipeline`]).

use std::collections::BTreeMap;

use loco_rs::app::AppContext;
use pnex_core::site_recipe::{RunEngine, RunExtract, SiteRecipeRun};
use sea_orm::{ConnectionTrait, DatabaseBackend, Statement};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::{browser, insert_run, parse, NewRun, RecipeError, StartError};

/// Why a default rule was played, and where its outcome goes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Purpose {
    /// A contact's connections list (D976): extracts `connections.<n>`
    /// (cards), filtered and erased by the server once read.
    ConnectionsCapture {
        person_id: String,
        user_id: i64,
        url: String,
    },
    /// The `browse` step of a pipeline's row (pipeline-engine.md D995): the
    /// extracts resume the row.
    Pipeline {
        pipeline: String,
        item: String,
        /// The member whose agent plays it: rows the run asks again play
        /// in the same agent.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        user: Option<i64>,
    },
}

impl Purpose {
    pub fn kind(&self) -> &'static str {
        match self {
            // The run's `trigger` holds 16 chars; runs are found by the tag.
            Self::ConnectionsCapture { .. } => "connections",
            Self::Pipeline { .. } => "pipeline",
        }
    }
}

fn stmt(sql: &str, values: Vec<sea_orm::Value>) -> Statement {
    Statement::from_sql_and_values(DatabaseBackend::Postgres, sql, values)
}

/// Starts a default rule for `purpose` on the engine the rule asks for.
pub async fn start(
    ctx: &AppContext,
    org_id: i64,
    user_id: Option<i64>,
    yaml: &str,
    params: BTreeMap<String, String>,
    purpose: &Purpose,
) -> Result<SiteRecipeRun, StartError> {
    let recipe = parse(yaml).map_err(|v| StartError::Recipe(RecipeError::Invalid(v)))?;
    if let Some(missing) = recipe.params.iter().find(|p| !params.contains_key(*p)) {
        return Err(StartError::ParamMissing(missing.clone()));
    }
    let engine = recipe.engine(None);
    if engine == RunEngine::Extension {
        super::check_extension_run(&ctx.db, org_id).await?;
    }
    let purpose_json = serde_json::to_value(purpose).unwrap_or_default();
    let new = NewRun {
        recipe_id: None,
        recipe_version: None,
        recipe: &recipe,
        yaml,
        params: &params,
        trigger: purpose.kind(),
        user_id,
        engine,
        purpose: Some(purpose_json),
    };
    let run = insert_run(&ctx.db, org_id, &new)
        .await
        .map_err(|e| StartError::Recipe(RecipeError::Db(e)))?;
    if engine == RunEngine::ServerBrowser {
        let run_id = Uuid::parse_str(&run.id).expect("uuid");
        browser::spawn(ctx.clone(), org_id, run_id, recipe, params, None, true);
    }
    Ok(run)
}

/// Acts on the end of a run started for a purpose; nothing for any other.
pub async fn on_finished(ctx: &AppContext, org_id: i64, run_id: Uuid) {
    let row = ctx
        .db
        .query_one_raw(stmt(
            "SELECT a.plan, a.extracts, j.state FROM agent_runs a
               JOIN runner_jobs j ON j.id = a.job_id
             WHERE a.id = $1 AND a.org_id = $2",
            vec![run_id.into(), org_id.into()],
        ))
        .await
        .ok()
        .flatten();
    let Some(row) = row else {
        return;
    };
    let plan: serde_json::Value = row.try_get("", "plan").unwrap_or_default();
    let Ok(purpose) = serde_json::from_value::<Purpose>(plan["purpose"].clone()) else {
        return;
    };
    let extracts: Vec<RunExtract> = row
        .try_get::<serde_json::Value>("", "extracts")
        .ok()
        .and_then(|v| serde_json::from_value(v).ok())
        .unwrap_or_default();
    let ok = row.try_get::<String>("", "state").unwrap_or_default() == "succeeded";
    let outcome = match &purpose {
        Purpose::ConnectionsCapture {
            person_id, user_id, ..
        } => {
            crate::services::network_connections::captured(
                ctx, org_id, run_id, person_id, *user_id, ok, &extracts,
            )
            .await
        }
        Purpose::Pipeline {
            pipeline,
            item,
            user,
        } => {
            let texts = ok.then(|| {
                extracts
                    .iter()
                    .map(|e| match e.cards.is_empty() {
                        true => (e.name.clone(), e.text.clone()),
                        // Cards reach the pipeline as a JSON array of records.
                        false => (
                            e.name.clone(),
                            serde_json::to_string(&e.cards).unwrap_or_default(),
                        ),
                    })
                    .collect()
            });
            match crate::services::pipeline_engine::store::resume_row(
                ctx, org_id, pipeline, item, *user, texts,
            )
            .await
            {
                Ok(true) => Ok(()),
                // The row read them: nothing of the navigation stays (D995).
                Ok(false) => ctx
                    .db
                    .execute_raw(stmt(
                        "UPDATE agent_runs SET extracts = '[]' WHERE id = $1 AND org_id = $2",
                        vec![run_id.into(), org_id.into()],
                    ))
                    .await
                    .map(|_| ())
                    .map_err(Into::into),
                Err(e) => {
                    tracing::warn!(run = %run_id, code = e.code(), "pipeline row not resumed");
                    Ok(())
                }
            }
        }
    };
    if let Err(e) = outcome {
        tracing::warn!(run = %run_id, error = %e, "outcome of a default rule not applied");
    }
}

/// Runs started for `kind` by the org, newest first: (run id, purpose,
/// job state, reason, created at).
pub async fn runs_of(
    db: &impl ConnectionTrait,
    org_id: i64,
    kind: &str,
    key: &str,
    value: &str,
) -> Result<Vec<(Uuid, Purpose, String, Option<String>, String)>, sea_orm::DbErr> {
    let rows = db
        .query_all_raw(stmt(
            "SELECT a.id, a.plan -> 'purpose' AS purpose, j.state, a.reason, a.created_at
             FROM agent_runs a JOIN runner_jobs j ON j.id = a.job_id
             WHERE a.org_id = $1 AND a.plan -> 'purpose' ->> 'kind' = $2
               AND a.plan -> 'purpose' ->> $3 = $4
             ORDER BY a.created_at DESC LIMIT 20",
            vec![org_id.into(), kind.into(), key.into(), value.into()],
        ))
        .await?;
    Ok(rows
        .iter()
        .filter_map(|r| {
            let purpose = serde_json::from_value(r.try_get("", "purpose").ok()?).ok()?;
            Some((
                r.try_get("", "id").ok()?,
                purpose,
                r.try_get("", "state").ok()?,
                r.try_get("", "reason").ok().flatten(),
                r.try_get::<chrono::DateTime<chrono::FixedOffset>>("", "created_at")
                    .ok()?
                    .to_rfc3339(),
            ))
        })
        .collect())
}

/// Navigations of the org's pipelines whose key starts with `prefix`, for
/// rows whose column `param` is `value`, newest first: (run id, the row's
/// params, job state, reason, created at).
pub async fn pipeline_runs(
    db: &impl ConnectionTrait,
    org_id: i64,
    prefix: &str,
    param: &str,
    value: &str,
) -> Result<
    Vec<(
        Uuid,
        BTreeMap<String, String>,
        String,
        Option<String>,
        String,
    )>,
    sea_orm::DbErr,
> {
    let rows = db
        .query_all_raw(stmt(
            "SELECT a.id, a.plan -> 'params' AS params, j.state, a.reason, a.created_at
             FROM agent_runs a JOIN runner_jobs j ON j.id = a.job_id
             WHERE a.org_id = $1 AND a.plan -> 'purpose' ->> 'kind' = 'pipeline'
               AND a.plan -> 'purpose' ->> 'pipeline' LIKE $2
               AND a.plan -> 'params' ->> $3 = $4
             ORDER BY a.created_at DESC LIMIT 20",
            vec![
                org_id.into(),
                format!("{prefix}%").into(),
                param.into(),
                value.into(),
            ],
        ))
        .await?;
    Ok(rows
        .iter()
        .filter_map(|r| {
            let params = serde_json::from_value(r.try_get("", "params").ok()?).ok()?;
            Some((
                r.try_get("", "id").ok()?,
                params,
                r.try_get("", "state").ok()?,
                r.try_get("", "reason").ok().flatten(),
                r.try_get::<chrono::DateTime<chrono::FixedOffset>>("", "created_at")
                    .ok()?
                    .to_rfc3339(),
            ))
        })
        .collect())
}

/// Navigations of the org's pipelines whose key starts with `prefix`, over
/// the last 24 h, for rows whose `url` starts with `url_prefix`.
pub async fn pipeline_runs_today(
    db: &impl ConnectionTrait,
    org_id: i64,
    prefix: &str,
    url_prefix: &str,
) -> Result<i64, sea_orm::DbErr> {
    Ok(db
        .query_one_raw(stmt(
            "SELECT count(*) AS n FROM agent_runs
             WHERE org_id = $1 AND plan -> 'purpose' ->> 'kind' = 'pipeline'
               AND plan -> 'purpose' ->> 'pipeline' LIKE $2
               AND plan -> 'params' ->> 'url' LIKE $3
               AND created_at > now() - interval '24 hours'",
            vec![
                org_id.into(),
                format!("{prefix}%").into(),
                format!("{url_prefix}%").into(),
            ],
        ))
        .await?
        .and_then(|r| r.try_get::<i64>("", "n").ok())
        .unwrap_or(0))
}

/// Runs started for `kind` by the org over the last 24 h whose purpose
/// `url` starts with `prefix` (the daily caps of D333, D779).
pub async fn count_today(
    db: &impl ConnectionTrait,
    org_id: i64,
    kind: &str,
    prefix: &str,
) -> Result<i64, sea_orm::DbErr> {
    Ok(db
        .query_one_raw(stmt(
            "SELECT count(*) AS n FROM agent_runs
             WHERE org_id = $1 AND plan -> 'purpose' ->> 'kind' = $2
               AND plan -> 'purpose' ->> 'url' LIKE $3
               AND created_at > now() - interval '24 hours'",
            vec![org_id.into(), kind.into(), format!("{prefix}%").into()],
        ))
        .await?
        .and_then(|r| r.try_get::<i64>("", "n").ok())
        .unwrap_or(0))
}
