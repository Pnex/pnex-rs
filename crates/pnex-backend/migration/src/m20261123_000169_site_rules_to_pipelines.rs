//! A site rule becomes an org pipeline (pipelines.md PE5.5, Shan's choice of
//! 2026-11-23): the start gives its parameters as one row, then its
//! navigation plays. Each rule keeps its id, so the `agent_run` nodes and the
//! navigation journal still name it, and each version becomes a version of
//! the pipeline. Parameters are lower-cased: they are now columns.

use sea_orm_migration::prelude::*;
use sea_orm_migration::sea_orm::{ConnectionTrait, DatabaseBackend, Statement};
use serde_json::{json, Value};

#[derive(DeriveMigrationName)]
pub struct Migration;

/// Host of an address or host (`https://www.x.org/a` → `www.x.org`).
fn host(raw: &str) -> String {
    let s = raw.trim();
    let s = s.split_once("://").map_or(s, |(_, rest)| rest);
    s.split(['/', '?', '#', ':'])
        .next()
        .unwrap_or_default()
        .to_ascii_lowercase()
}

/// The pipeline document of a rule's YAML, as JSON (valid YAML); `None`
/// when the YAML is not a rule.
fn document(yaml: &str) -> Option<String> {
    let rule: Value = serde_saphyr::from_str(yaml).ok()?;
    let site = rule["site"].as_str()?.trim().to_string();
    let params: Vec<String> = rule["params"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default();
    let mut steps = serde_json::to_string(rule.get("steps")?).ok()?;
    for p in &params {
        steps = steps.replace(
            &format!("{{{{{p}}}}}"),
            &format!("{{{{{}}}}}", p.to_lowercase()),
        );
    }
    let params: Vec<String> = params.iter().map(|p| p.to_lowercase()).collect();
    let (first, rest) = match params.split_first() {
        Some((p, rest)) => (p.clone(), rest.to_vec()),
        None => ("run".to_string(), Vec::new()),
    };
    let mut hosts = vec![host(&site).trim_start_matches("www.").to_string()];
    for h in rule["allow_hosts"].as_array().into_iter().flatten() {
        let h = host(h.as_str().unwrap_or_default());
        if !h.is_empty() && !hosts.contains(&h) {
            hosts.push(h);
        }
    }
    let mut browse = json!({
        "as": "page", "for_each": "start", "site": site,
        "steps": serde_json::from_str::<Value>(&steps).ok()?,
    });
    for k in ["session", "browser", "captures"] {
        if let Some(v) = rule[k].as_str() {
            browse[k] = json!(v);
        }
    }
    let doc = json!({
        "key": "site_rule",
        "name": rule["name"],
        "hosts": hosts,
        "steps": [
            {"enumerate": {"as": "start", "track": true,
                "cross": [{"as": first, "asked": rest}]}},
            {"browse": browse},
        ],
    });
    serde_json::to_string_pretty(&doc).ok()
}

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, m: &SchemaManager) -> Result<(), DbErr> {
        // Done once: a round trip down and up finds the rules already moved.
        if !m.has_table("site_recipes").await? {
            return Ok(());
        }
        let db = m.get_connection();
        let pg = |sql: &str, values: Vec<sea_orm_migration::sea_orm::Value>| {
            Statement::from_sql_and_values(DatabaseBackend::Postgres, sql, values)
        };
        let rules = db
            .query_all_raw(pg(
                "SELECT id, org_id, site, name, yaml, version, created_at, updated_at \
                 FROM site_recipes",
                vec![],
            ))
            .await?;
        for r in rules {
            let id: sea_orm_migration::sea_orm::prelude::Uuid = r.try_get("", "id")?;
            let org: i64 = r.try_get("", "org_id")?;
            let site: String = r.try_get("", "site")?;
            let name: String = r.try_get("", "name")?;
            let yaml: String = r.try_get("", "yaml")?;
            let Some(doc) = document(&yaml) else {
                continue;
            };
            // A pipeline name is unique in the org, a rule's per site: the
            // pipeline carries both, as the Web sites page names new ones.
            let taken = |n: String| {
                pg(
                    "SELECT 1 AS x FROM pipelines WHERE org_id = $1 AND name = $2",
                    vec![org.into(), n.into()],
                )
            };
            let mut label = format!("{name} · {site}");
            if db.query_one_raw(taken(label.clone())).await?.is_some() {
                label = format!("{name} · {id}");
            }
            db.execute_raw(pg(
                "INSERT INTO pipelines (id, org_id, name, version, document, created_at, updated_at) \
                 SELECT id, org_id, $2, version, $3, created_at, updated_at FROM site_recipes WHERE id = $1",
                vec![id.into(), label.into(), doc.clone().into()],
            ))
            .await?;
            let versions = db
                .query_all_raw(pg(
                    "SELECT version, yaml, created_by, created_at FROM site_recipe_versions \
                     WHERE recipe_id = $1",
                    vec![id.into()],
                ))
                .await?;
            for v in versions {
                let version: i32 = v.try_get("", "version")?;
                let old: String = v.try_get("", "yaml")?;
                let author: Option<i64> = v.try_get("", "created_by")?;
                let at: sea_orm_migration::sea_orm::prelude::DateTimeWithTimeZone =
                    v.try_get("", "created_at")?;
                // An old version that no longer reads is kept as the current one.
                let doc = document(&old).unwrap_or_else(|| doc.clone());
                db.execute_raw(pg(
                    "INSERT INTO pipeline_versions (pipeline_id, version, document, author_id, created_at) \
                     VALUES ($1, $2, $3, (SELECT u.id FROM users u WHERE u.id = $4), $5) \
                     ON CONFLICT DO NOTHING",
                    vec![id.into(), version.into(), doc.into(), author.into(), at.into()],
                ))
                .await?;
            }
            db.execute_raw(pg(
                "INSERT INTO pipeline_versions (pipeline_id, version, document) \
                 SELECT id, version, document FROM pipelines WHERE id = $1 ON CONFLICT DO NOTHING",
                vec![id.into()],
            ))
            .await?;
        }
        db.execute_unprepared(
            "UPDATE agent_runs a SET recipe_id = NULL WHERE recipe_id IS NOT NULL \
               AND NOT EXISTS (SELECT 1 FROM pipelines p WHERE p.id = a.recipe_id);
             ALTER TABLE agent_runs DROP CONSTRAINT IF EXISTS agent_runs_recipe_id_fkey;
             ALTER TABLE agent_runs ADD CONSTRAINT agent_runs_recipe_id_fkey
               FOREIGN KEY (recipe_id) REFERENCES pipelines(id) ON DELETE SET NULL;
             DROP TABLE site_recipe_versions;
             DROP TABLE site_recipes;",
        )
        .await?;
        Ok(())
    }

    async fn down(&self, _m: &SchemaManager) -> Result<(), DbErr> {
        // Gone for good: a rule is now an org pipeline.
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::document;
    use serde_json::{json, Value};

    #[test]
    fn a_rule_becomes_an_asked_row_then_its_navigation() {
        let yaml = "name: Programme\nsite: www.example.org\nparams: [Code]\n\
            allow_hosts: [\"https://files.example.net/x\"]\ncaptures: \"days:7\"\n\
            steps:\n  - open: \"/p/{{Code}}\"\n";
        let doc: Value = serde_json::from_str(&document(yaml).unwrap()).unwrap();
        assert_eq!(
            doc,
            json!({
                "key": "site_rule", "name": "Programme",
                "hosts": ["example.org", "files.example.net"],
                "steps": [
                    {"enumerate": {"as": "start", "track": true,
                        "cross": [{"as": "code", "asked": []}]}},
                    {"browse": {"as": "page", "for_each": "start", "site": "www.example.org",
                        "captures": "days:7", "steps": [{"open": "/p/{{code}}"}]}},
                ],
            })
        );
        assert_eq!(document("name: x\n"), None);
    }
}
