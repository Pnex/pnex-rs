//! Repairs the flow versions that migration 000162 rewrote: it wrote
//! `"inputs": null` on every node without input annotations, and such a node
//! no longer reads (`flow-node-unreadable`). The null key is removed.

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, m: &SchemaManager) -> Result<(), DbErr> {
        m.get_connection()
            .execute_unprepared(
                r#"UPDATE flow_versions SET graph = jsonb_set(graph, '{nodes}', (
                    SELECT jsonb_agg(CASE WHEN n->'inputs' = 'null'::jsonb THEN n - 'inputs' ELSE n END ORDER BY i)
                    FROM jsonb_array_elements(graph->'nodes') WITH ORDINALITY AS t(n, i)))
                WHERE jsonb_path_exists(graph, '$.nodes[*] ? (@.inputs == null)')"#,
            )
            .await?;
        Ok(())
    }

    async fn down(&self, _m: &SchemaManager) -> Result<(), DbErr> {
        // A null `inputs` never reads: nothing to restore.
        Ok(())
    }
}
