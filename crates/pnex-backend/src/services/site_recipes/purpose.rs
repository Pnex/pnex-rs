//! What the server does when a run of a default rule ends (agent-unification
//! D780, lot U4). A default rule is played like any rule; the purpose it was
//! started for is kept in its plan, and acting on the outcome is the server's
//! job: never a kind of mission, never the agent. A profile capture becomes
//! a document of the person (D333), an employee list one of the company
//! (D779); both are purged after 30 days with the other captured texts.

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
    /// One profile page of a person (D333): extract `profile`.
    ProfileCapture { person_id: String, url: String },
    /// One company's employee list (D779): extract `people`.
    PeopleCapture {
        organization_id: String,
        url: String,
    },
}

impl Purpose {
    pub fn kind(&self) -> &'static str {
        match self {
            Self::ProfileCapture { .. } => "profile_capture",
            Self::PeopleCapture { .. } => "people_capture",
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
    let text = |name: &str| {
        extracts
            .iter()
            .find(|e| e.name == name)
            .map(|e| e.text.clone())
            .filter(|t| !t.trim().is_empty())
    };
    let source = format!("rule_run:{run_id}");
    let outcome = match &purpose {
        Purpose::ProfileCapture { person_id, url } => match text("profile").filter(|_| ok) {
            Some(t) => {
                crate::services::profiles::profile_captured(
                    ctx, org_id, &source, person_id, url, &t,
                )
                .await
            }
            None => Ok(()),
        },
        Purpose::PeopleCapture {
            organization_id,
            url,
        } => match text("people").filter(|_| ok) {
            Some(t) => {
                crate::services::profiles::people_captured(
                    ctx,
                    org_id,
                    &source,
                    organization_id,
                    url,
                    &t,
                )
                .await
            }
            None => Ok(()),
        },
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
