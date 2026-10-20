//! `site_recipe` flow node (site-recipes.md D327). The flow runtime is not
//! trusted: each call names its deployed node, whose recipe is read from
//! the deployed version, in the node's org. The run is journaled like a
//! studio run (trigger `flow`) and waited for.

use std::collections::BTreeMap;

use loco_rs::app::AppContext;
use pnex_core::err_codes;
use pnex_core::flow::FlowNodeKind;
use pnex_core::site_recipe::{
    RunStatus, SiteRecipeNodeReply, SiteRecipeNodeRequest, NODE_PORT_BROKE, NODE_PORT_FAILED,
    NODE_PORT_OK, STRING_MAX,
};
use sea_orm::{ConnectionTrait, DatabaseConnection};
use serde_json::{json, Value};
use uuid::Uuid;

use super::{browser, stmt, NewRun, RecipeError};
use crate::services::ontology::flow_nodes::{deployed, NodeError};

fn refused(code: &str, args: Value) -> SiteRecipeNodeReply {
    SiteRecipeNodeReply {
        port: NODE_PORT_FAILED,
        code: Some(code.to_string()),
        args,
        ..Default::default()
    }
}

/// Plays the rule of the deployed node `req.node` and answers its outcome.
pub async fn run(
    ctx: &AppContext,
    req: &SiteRecipeNodeRequest,
) -> Result<SiteRecipeNodeReply, NodeError> {
    let FlowNodeKind::SiteRecipe { config } = deployed(&ctx.db, &req.node).await? else {
        return Err(NodeError::NotDeployed);
    };
    let org_id = req.node.org_id;
    let unavailable = || refused(err_codes::SITE_RECIPE_UNAVAILABLE, Value::Null);
    let Ok(recipe_id) = Uuid::parse_str(&config.recipe_id) else {
        return Ok(refused(err_codes::SITE_RECIPE_NOT_FOUND, Value::Null));
    };
    let saved = match super::get(&ctx.db, org_id, recipe_id).await {
        Ok(r) => r,
        Err(RecipeError::NotFound) => {
            return Ok(refused(err_codes::SITE_RECIPE_NOT_FOUND, Value::Null))
        }
        Err(_) => return Ok(unavailable()),
    };
    let Ok(recipe) = super::parse(&saved.yaml) else {
        return Ok(refused(err_codes::SITE_RECIPE_INVALID, Value::Null));
    };
    let mut params = config.params.clone();
    params.extend(req.params.clone());
    let params: BTreeMap<String, String> = params
        .into_iter()
        .filter(|(k, _)| recipe.params.contains(k))
        .map(|(k, v)| (k, v.chars().take(STRING_MAX).collect()))
        .collect();
    if let Some(missing) = recipe.params.iter().find(|p| !params.contains_key(*p)) {
        return Ok(refused(
            err_codes::SITE_RECIPE_PARAM_MISSING,
            json!({ "param": missing }),
        ));
    }
    let new = NewRun {
        recipe_id: Some(recipe_id),
        recipe_version: Some(saved.version),
        recipe: &recipe,
        yaml: &saved.yaml,
        params: &params,
        trigger: "flow",
        user_id: None,
    };
    let Ok(started) = super::insert_run(&ctx.db, org_id, &new).await else {
        return Ok(unavailable());
    };
    let run_id = Uuid::parse_str(&started.id).expect("uuid");
    let status = browser::play_to_end(ctx.clone(), org_id, run_id, recipe, params).await;
    let Ok(view) = super::run(&ctx.db, org_id, run_id).await else {
        return Ok(unavailable());
    };
    let previous = previous_flow_status(&ctx.db, org_id, recipe_id, run_id)
        .await
        .unwrap_or(None);
    let port = match (status, previous) {
        (RunStatus::Ok, _) => NODE_PORT_OK,
        (_, Some(RunStatus::Ok)) => NODE_PORT_BROKE,
        _ => NODE_PORT_FAILED,
    };
    let failed_step = view.failed_step.map(|i| {
        let action = view
            .steps
            .iter()
            .find(|s| s.index == i)
            .map(|s| s.action.clone());
        json!({ "index": i, "action": action, "reason": view.reason })
    });
    let outputs: Vec<Value> = view
        .extracts
        .iter()
        .map(|e| {
            json!({
                "name": e.name,
                "kind": e.kind,
                "sha256": e.sha256,
                "chars": e.chars,
                "changed": view.changes.contains_key(&e.name),
                "from": e.from,
            })
        })
        .collect();
    let manifest = json!({
        "site": view.site,
        "recipe": view.name,
        "recipe_id": recipe_id,
        "version": saved.version,
        "run": view.id,
        "status": view.status,
        "reason": if view.failed_step.is_none() { view.reason.clone() } else { None },
        "failed_step": failed_step,
        "outputs": outputs,
    });
    let payload = view
        .extracts
        .into_iter()
        .map(|e| (e.name, e.text))
        .collect();
    Ok(SiteRecipeNodeReply {
        port,
        manifest,
        payload,
        code: None,
        args: Value::Null,
    })
}

/// Status of the previous finished flow run of the recipe (studio runs
/// play drafts: they do not count).
async fn previous_flow_status(
    db: &DatabaseConnection,
    org_id: i64,
    recipe_id: Uuid,
    run_id: Uuid,
) -> Result<Option<RunStatus>, sea_orm::DbErr> {
    let row = db
        .query_one_raw(stmt(
            "SELECT status FROM site_recipe_runs WHERE org_id = $1 AND recipe_id = $2 \
             AND trigger = 'flow' AND id <> $3 AND finished_at IS NOT NULL \
             AND started_at <= (SELECT started_at FROM site_recipe_runs WHERE id = $3) \
             ORDER BY started_at DESC LIMIT 1",
            vec![org_id.into(), recipe_id.into(), run_id.into()],
        ))
        .await?;
    Ok(row
        .and_then(|r| r.try_get::<String>("", "status").ok())
        .map(|s| RunStatus::parse(&s)))
}

/// Deploy gate: every `site_recipe` node names a saved rule of the org.
pub async fn deploy_violations(
    db: &DatabaseConnection,
    org_id: i64,
    graph: &pnex_core::flow::FlowGraph,
) -> Result<Vec<pnex_core::FlowViolation>, sea_orm::DbErr> {
    let mut v = Vec::new();
    for n in &graph.nodes {
        let FlowNodeKind::SiteRecipe { config } = &n.kind else {
            continue;
        };
        if let Some((code, message)) = config.check_deployable() {
            v.push(pnex_core::FlowViolation::new(
                Some(n.id.as_str()),
                code,
                message,
            ));
            continue;
        }
        let Ok(id) = Uuid::parse_str(&config.recipe_id) else {
            continue;
        };
        let found = db
            .query_one_raw(stmt(
                "SELECT 1 AS x FROM site_recipes WHERE org_id = $1 AND id = $2",
                vec![org_id.into(), id.into()],
            ))
            .await?;
        if found.is_none() {
            v.push(pnex_core::FlowViolation::new(
                Some(n.id.as_str()),
                "site_recipe_unknown",
                "the navigation rule of this node does not exist in the organization",
            ));
        }
    }
    Ok(v)
}

/// Next cron times listed per recipe.
const NEXT_MAX: usize = 5;

/// Triggers reaching the recipe's nodes in a graph: (cron expressions,
/// repeat intervals). Walks the wires back from each `site_recipe` node
/// playing `recipe_id`.
fn triggers(graph: &pnex_core::flow::FlowGraph, recipe_id: &str) -> (Vec<String>, Vec<f64>) {
    use std::collections::HashSet;
    let mut reached: HashSet<&str> = graph
        .nodes
        .iter()
        .filter(|n| matches!(&n.kind, FlowNodeKind::SiteRecipe { config } if config.recipe_id == recipe_id))
        .map(|n| n.id.as_str())
        .collect();
    loop {
        let before = reached.len();
        for n in &graph.nodes {
            if n.outputs
                .iter()
                .any(|w| w.targets.iter().any(|t| reached.contains(t.as_str())))
            {
                reached.insert(n.id.as_str());
            }
        }
        if reached.len() == before {
            break;
        }
    }
    let (mut cron, mut every) = (Vec::new(), Vec::new());
    for n in graph
        .nodes
        .iter()
        .filter(|n| reached.contains(n.id.as_str()))
    {
        if let FlowNodeKind::Inject { config } = &n.kind {
            if !config.cron.trim().is_empty() {
                cron.push(config.cron.trim().to_string());
            }
            if let Some(s) = config.repeat_secs.filter(|s| *s > 0.0) {
                every.push(s);
            }
        }
    }
    (cron, every)
}

/// Next occurrences of cron expressions after `now`, soonest first
/// (5 or 6 fields, UTC like the flow runtime).
fn next_times(
    crons: &[String],
    now: chrono::DateTime<chrono::Utc>,
) -> Vec<chrono::DateTime<chrono::Utc>> {
    let parser = croner::parser::CronParser::builder()
        .seconds(croner::parser::Seconds::Optional)
        .build();
    // Whole seconds: the occurrences keep the start's sub-second part.
    let now = chrono::Timelike::with_nanosecond(&now, 0).unwrap_or(now);
    let mut out: Vec<_> = crons
        .iter()
        .filter_map(|c| parser.parse(c).ok())
        .flat_map(|c| c.iter_after(now).take(NEXT_MAX).collect::<Vec<_>>())
        .collect();
    out.sort();
    out.dedup();
    out.truncate(NEXT_MAX);
    out
}

/// Deployed flows of the org playing the recipe, and their next runs.
pub async fn schedule(
    db: &DatabaseConnection,
    org_id: i64,
    recipe_id: Uuid,
) -> Result<pnex_core::site_recipe::SiteRecipeSchedule, sea_orm::DbErr> {
    use crate::models::_entities::{flow_versions, flows};
    use sea_orm::{ColumnTrait, EntityTrait, QueryFilter};
    let deployed = flows::Entity::find()
        .filter(flows::Column::OrgId.eq(org_id))
        .filter(flows::Column::Status.eq(pnex_core::FLOW_STATUS_DEPLOYED))
        .all(db)
        .await?;
    let version_ids: Vec<i64> = deployed
        .iter()
        .filter_map(|f| f.deployed_version_id)
        .collect();
    let mut graphs: std::collections::HashMap<i64, serde_json::Value> =
        flow_versions::Entity::find()
            .filter(flow_versions::Column::Id.is_in(version_ids))
            .all(db)
            .await?
            .into_iter()
            .map(|v| (v.id, v.graph))
            .collect();
    let id = recipe_id.to_string();
    let mut out = pnex_core::site_recipe::SiteRecipeSchedule::default();
    let mut crons = Vec::new();
    for flow in deployed {
        let Some(raw) = flow.deployed_version_id.and_then(|v| graphs.remove(&v)) else {
            continue;
        };
        let Ok(graph) = serde_json::from_value::<pnex_core::flow::FlowGraph>(raw) else {
            continue;
        };
        if !graph.nodes.iter().any(
            |n| matches!(&n.kind, FlowNodeKind::SiteRecipe { config } if config.recipe_id == id),
        ) {
            continue;
        }
        let (cron, every_secs) = triggers(&graph, &id);
        crons.extend(cron.iter().cloned());
        out.flows.push(pnex_core::site_recipe::ScheduledFlow {
            flow_id: flow.id,
            name: flow.name,
            cron,
            every_secs,
        });
    }
    out.next = next_times(&crons, chrono::Utc::now())
        .into_iter()
        .map(|t| t.to_rfc3339())
        .collect();
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    #[test]
    fn triggers_are_found_upstream_of_the_recipe_node() {
        let graph: pnex_core::flow::FlowGraph = serde_json::from_value(json!({"nodes": [
            {"id": "i1", "kind": "inject", "config": {"cron": "0 6 * * *"}, "outputs": [{"port": 0, "targets": ["f"]}]},
            {"id": "i2", "kind": "inject", "config": {"repeat_secs": 3600.0}, "outputs": [{"port": 0, "targets": ["s"]}]},
            {"id": "i3", "kind": "inject", "config": {"cron": "*/5 * * * *"}, "outputs": [{"port": 0, "targets": ["d"]}]},
            {"id": "f", "kind": "debug", "outputs": [{"port": 0, "targets": ["s"]}]},
            {"id": "d", "kind": "debug"},
            {"id": "s", "kind": "site_recipe", "config": {"recipe_id": "r1"}}
        ]}))
        .expect("graph");
        let (cron, every) = triggers(&graph, "r1");
        assert_eq!(cron, vec!["0 6 * * *".to_string()]);
        assert_eq!(every, vec![3600.0]);
        assert_eq!(triggers(&graph, "other"), (vec![], vec![]));
    }

    #[test]
    fn next_times_merge_crons_soonest_first() {
        let now = chrono::Utc.with_ymd_and_hms(2026, 10, 20, 5, 0, 0).unwrap()
            + chrono::Duration::milliseconds(250);
        let next = next_times(
            &["0 6 * * *".into(), "30 0 6 * * *".into(), "bad".into()],
            now,
        );
        assert_eq!(next.len(), NEXT_MAX);
        assert_eq!(
            next[0],
            chrono::Utc.with_ymd_and_hms(2026, 10, 20, 6, 0, 0).unwrap()
        );
        assert_eq!(
            next[1],
            chrono::Utc
                .with_ymd_and_hms(2026, 10, 20, 6, 0, 30)
                .unwrap()
        );
    }
}
