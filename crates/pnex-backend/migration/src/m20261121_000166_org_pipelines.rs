//! One kind of pipeline (pipelines.md D934): an org's own pipeline is an
//! engine document. Each SQL transform of the former format becomes a `sql`
//! step that reads the same tables and a `write` that replaces its table,
//! with its checks and retention. A run keeps who started it; lineage keeps
//! the pack pipeline that wrote a table.

use std::collections::BTreeMap;

use sea_orm_migration::prelude::*;
use sea_orm_migration::sea_orm::{ConnectionTrait, DatabaseBackend, Statement};
use serde_json::{json, Value};

#[derive(DeriveMigrationName)]
pub struct Migration;

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct Former {
    transforms: Vec<Transform>,
}

#[derive(serde::Deserialize)]
struct Transform {
    output: String,
    inputs: BTreeMap<String, String>,
    sql: String,
    #[serde(default)]
    checks: Option<Value>,
    #[serde(default)]
    retain_days: Option<u16>,
}

/// The engine document of a former one, as JSON (valid YAML); `None` when
/// the document is not of the former format.
fn engine(name: &str, yaml: &str) -> Option<String> {
    let former: Former = serde_saphyr::from_str(yaml).ok()?;
    let mut steps = Vec::new();
    for (i, t) in former.transforms.into_iter().enumerate() {
        let rows = format!("t{}", i + 1);
        steps.push(json!({ "sql": { "as": rows, "tables": t.inputs, "sql": t.sql } }));
        let mut table = json!({ "name": t.output, "replace": true });
        if let Some(c) = t
            .checks
            .filter(|c| c.as_object().is_some_and(|m| !m.is_empty()))
        {
            table["checks"] = c;
        }
        if let Some(d) = t.retain_days {
            table["retain_days"] = json!(d);
        }
        steps.push(json!({ "write": { "from": rows, "table": table } }));
    }
    let doc = json!({ "key": "org_pipeline", "name": name, "steps": steps });
    serde_json::to_string_pretty(&doc).ok()
}

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, m: &SchemaManager) -> Result<(), DbErr> {
        let db = m.get_connection();
        db.execute_unprepared(
            "ALTER TABLE pipeline_jobs
               ADD COLUMN started_by bigint REFERENCES users(id) ON DELETE SET NULL;
             ALTER TABLE dataset_transaction_inputs ADD COLUMN pipeline_key text;",
        )
        .await?;
        let rows = db
            .query_all_raw(Statement::from_string(
                DatabaseBackend::Postgres,
                "SELECT p.id, p.name, v.version, v.document FROM pipelines p \
                 JOIN pipeline_versions v ON v.pipeline_id = p.id",
            ))
            .await?;
        for r in rows {
            let id: sea_orm_migration::sea_orm::prelude::Uuid = r.try_get("", "id")?;
            let name: String = r.try_get("", "name")?;
            let version: i32 = r.try_get("", "version")?;
            let document: String = r.try_get("", "document")?;
            let Some(doc) = engine(&name, &document) else {
                continue;
            };
            db.execute_raw(Statement::from_sql_and_values(
                DatabaseBackend::Postgres,
                "UPDATE pipeline_versions SET document = $3 WHERE pipeline_id = $1 AND version = $2",
                [id.into(), version.into(), doc.clone().into()],
            ))
            .await?;
            db.execute_raw(Statement::from_sql_and_values(
                DatabaseBackend::Postgres,
                "UPDATE pipelines SET document = $3 WHERE id = $1 AND version = $2",
                [id.into(), version.into(), doc.into()],
            ))
            .await?;
        }
        Ok(())
    }

    async fn down(&self, m: &SchemaManager) -> Result<(), DbErr> {
        // The documents stay engine documents: the former ones are not kept.
        m.get_connection()
            .execute_unprepared(
                "ALTER TABLE pipeline_jobs DROP COLUMN started_by;
                 ALTER TABLE dataset_transaction_inputs DROP COLUMN pipeline_key;",
            )
            .await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_transform_becomes_a_query_and_a_replacing_write() {
        let yaml = "transforms:\n  - output: Clean sales\n    inputs: {raw: Raw sales}\n    sql: SELECT * FROM raw\n    checks: {min_rows: 1}\n    retain_days: 30\n";
        let doc: Value = serde_saphyr::from_str(&engine("Sales", yaml).unwrap()).unwrap();
        assert_eq!(
            doc,
            json!({"key": "org_pipeline", "name": "Sales", "steps": [
                {"sql": {"as": "t1", "tables": {"raw": "Raw sales"}, "sql": "SELECT * FROM raw"}},
                {"write": {"from": "t1", "table": {"name": "Clean sales", "replace": true,
                  "checks": {"min_rows": 1}, "retain_days": 30}}}
            ]})
        );
        assert_eq!(
            engine("x", "key: a\nsteps: []\n"),
            None,
            "an engine document stays"
        );
    }
}
