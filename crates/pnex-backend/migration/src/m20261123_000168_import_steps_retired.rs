//! The imports' own `steps:` are retired (pipelines.md D344, Shan's choice of
//! 2026-11-22): an org's pipeline fills a Table, its import reads it. The
//! steps leave each mapping; such an import keeps its objects but has no
//! source left, so its schedule stops.

use sea_orm_migration::prelude::*;
use sea_orm_migration::sea_orm::{ConnectionTrait, Statement};

#[derive(DeriveMigrationName)]
pub struct Migration;

/// The mapping without its top-level `steps:` block.
fn without_steps(yaml: &str) -> Option<String> {
    let mut out = Vec::new();
    let (mut skipping, mut found) = (false, false);
    for line in yaml.lines() {
        if line.starts_with("steps:") {
            (skipping, found) = (true, true);
            continue;
        }
        // The block goes on while lines are indented, list items or blank.
        if skipping && (line.starts_with([' ', '\t', '-']) || line.trim().is_empty()) {
            continue;
        }
        skipping = false;
        out.push(line);
    }
    found.then(|| out.join("\n") + "\n")
}

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, m: &SchemaManager) -> Result<(), DbErr> {
        let db = m.get_connection();
        let rows = db
            .query_all_raw(Statement::from_string(
                m.get_database_backend(),
                "SELECT id, mapping_yaml FROM object_imports WHERE mapping_yaml ~ '(^|\\n)steps:'",
            ))
            .await?;
        for row in rows {
            let id: sea_orm_migration::sea_orm::prelude::Uuid = row.try_get("", "id")?;
            let yaml: String = row.try_get("", "mapping_yaml")?;
            let Some(yaml) = without_steps(&yaml) else {
                continue;
            };
            db.execute_raw(Statement::from_sql_and_values(
                m.get_database_backend(),
                "UPDATE object_imports SET mapping_yaml = $1, recurrence = NULL, \
                   next_run_at = NULL, updated_at = now() WHERE id = $2",
                [yaml.into(), id.into()],
            ))
            .await?;
        }
        Ok(())
    }

    async fn down(&self, _m: &SchemaManager) -> Result<(), DbErr> {
        // Gone for good: their source is now an org's pipeline.
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::without_steps;

    #[test]
    fn only_the_steps_block_leaves() {
        let yaml = "key: t\nsteps:\n  - step: source_resolve\n    url: x\n- step: rows_filter\n\nformat: csv\nobjects: []\n";
        assert_eq!(
            without_steps(yaml).as_deref(),
            Some("key: t\nformat: csv\nobjects: []\n")
        );
        assert_eq!(without_steps("key: t\nformat: csv\n"), None);
        assert_eq!(
            without_steps("steps: []\nkey: t\n").as_deref(),
            Some("key: t\n")
        );
    }
}
