//! The `site_recipe` flow node folds into `agent_run` (agents.md D786). Every saved
//! flow version is rewired: the node becomes an `agent_run` playing the same rule, its
//! port 1 (failed, which held the refusals) stays on the refusals, and each wired port
//! becomes an `agent_run_finished` node filtered on the rule and the outcome
//! (0 passed, 1 failed, 2 broke) that feeds the same targets.

use sea_orm_migration::prelude::*;
use sea_orm_migration::sea_orm::{ConnectionTrait, DatabaseBackend, Statement};
use serde_json::{json, Value};

#[derive(DeriveMigrationName)]
pub struct Migration;

const OUTCOMES: [&str; 3] = ["passed", "failed", "broke"];

/// Rewires the `site_recipe` nodes of one graph; `false` when it has none.
fn fold(graph: &mut Value) -> bool {
    let Some(nodes) = graph.get_mut("nodes").and_then(Value::as_array_mut) else {
        return false;
    };
    let mut ids: Vec<String> = nodes
        .iter()
        .filter_map(|n| n["id"].as_str().map(str::to_string))
        .collect();
    let mut added = Vec::new();
    // (old node, old port) → new source node.
    let mut moved: Vec<(String, u64, String)> = Vec::new();
    let mut changed = false;
    for n in nodes.iter_mut() {
        if n["kind"] != "site_recipe" {
            continue;
        }
        changed = true;
        let id = n["id"].as_str().unwrap_or_default().to_string();
        let recipe_id = n["config"]["recipe_id"].clone();
        n["kind"] = json!("agent_run");
        let outputs = n["outputs"].as_array().cloned().unwrap_or_default();
        // Port 1 keeps the refusals: a run that cannot start never ends.
        let refused: Vec<Value> = outputs.iter().filter(|w| w["port"] == 1).cloned().collect();
        n["outputs"] = json!(refused);
        let (x, y) = (
            n["position"]["x"].as_f64().unwrap_or(0.0),
            n["position"]["y"].as_f64().unwrap_or(0.0),
        );
        for (i, w) in outputs.iter().enumerate() {
            let port = w["port"].as_u64().unwrap_or(0);
            let Some(outcome) = OUTCOMES.get(port as usize) else {
                continue;
            };
            let mut new_id = format!("{id}-{outcome}");
            while ids.contains(&new_id) {
                new_id.push('_');
            }
            ids.push(new_id.clone());
            added.push(json!({
                "id": new_id,
                "position": {"x": x, "y": y + 80.0 * (i as f64 + 1.0)},
                "kind": "agent_run_finished",
                "config": {"recipe_id": recipe_id, "outcome": outcome},
                "outputs": [{"port": 0, "targets": w["targets"]}],
            }));
            moved.push((id.clone(), port, new_id));
        }
    }
    if !changed {
        return false;
    }
    // Input-row annotations name the wire's source node and port.
    for n in nodes.iter_mut() {
        // `get_mut`, not `n["inputs"]`: indexing a missing key inserts a null.
        for input in n
            .get_mut("inputs")
            .and_then(Value::as_array_mut)
            .into_iter()
            .flatten()
        {
            let from = input["from"].as_str().unwrap_or_default().to_string();
            let port = input["from_port"].as_u64().unwrap_or(0);
            if let Some((_, _, new_id)) = moved.iter().find(|(o, p, _)| *o == from && *p == port) {
                input["from"] = json!(new_id);
                input["from_port"] = json!(0);
            }
        }
    }
    nodes.extend(added);
    true
}

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, m: &SchemaManager) -> Result<(), DbErr> {
        let db = m.get_connection();
        let rows = db
            .query_all_raw(Statement::from_string(
                DatabaseBackend::Postgres,
                "SELECT id, graph FROM flow_versions WHERE graph::text LIKE '%\"site_recipe\"%'",
            ))
            .await?;
        for r in rows {
            let id: i64 = r.try_get("", "id")?;
            let mut graph: Value = r.try_get("", "graph")?;
            if fold(&mut graph) {
                db.execute_raw(Statement::from_sql_and_values(
                    DatabaseBackend::Postgres,
                    "UPDATE flow_versions SET graph = $1 WHERE id = $2",
                    [graph.into(), id.into()],
                ))
                .await?;
            }
        }
        Ok(())
    }

    async fn down(&self, _m: &SchemaManager) -> Result<(), DbErr> {
        // The fold is not undone: the `site_recipe` node no longer exists.
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_site_recipe_node_becomes_an_agent_run_and_its_ends() {
        let mut graph = json!({"nodes": [
            {"id": "i", "kind": "inject", "outputs": [{"port": 0, "targets": ["s"]}]},
            {"id": "s", "kind": "site_recipe", "position": {"x": 10.0, "y": 20.0},
             "config": {"recipe_id": "r1", "params": {"code": "X"}},
             "outputs": [
                {"port": 0, "targets": ["ok"]},
                {"port": 1, "targets": ["ko"]},
                {"port": 2, "targets": ["warn"]}
             ]},
            {"id": "ok", "kind": "debug"},
            {"id": "ko", "kind": "debug"},
            {"id": "warn", "kind": "pnex_notify", "inputs": [{"pin": "v", "from": "s", "from_port": 2}]}
        ]});
        assert!(fold(&mut graph));
        let nodes = graph["nodes"].as_array().unwrap();
        let s = nodes.iter().find(|n| n["id"] == "s").unwrap();
        assert_eq!(s["kind"], "agent_run");
        assert_eq!(s["config"]["params"]["code"], "X");
        assert_eq!(s["outputs"], json!([{"port": 1, "targets": ["ko"]}]));
        let i = nodes.iter().find(|n| n["id"] == "i").unwrap();
        assert!(i.get("inputs").is_none());
        let ends: Vec<_> = nodes
            .iter()
            .filter(|n| n["kind"] == "agent_run_finished")
            .collect();
        assert_eq!(ends.len(), 3);
        let broke = nodes.iter().find(|n| n["id"] == "s-broke").unwrap();
        assert_eq!(
            broke["config"],
            json!({"recipe_id": "r1", "outcome": "broke"})
        );
        assert_eq!(broke["outputs"][0]["targets"], json!(["warn"]));
        let warn = nodes.iter().find(|n| n["id"] == "warn").unwrap();
        assert_eq!(
            warn["inputs"][0],
            json!({"pin": "v", "from": "s-broke", "from_port": 0})
        );
        let mut plain = json!({"nodes": [{"id": "d", "kind": "debug"}]});
        assert!(!fold(&mut plain));
    }
}
