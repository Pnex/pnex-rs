//! The cursor of each `connector` step of a pipeline (pipelines.md D794):
//! the last value a successful run read, and the incremental reads since the
//! last full one.

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, m: &SchemaManager) -> Result<(), DbErr> {
        m.get_connection()
            .execute_unprepared(
                "CREATE TABLE pipeline_cursors (
                   org_id bigint NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
                   pipeline text NOT NULL,
                   step text NOT NULL,
                   value text NOT NULL,
                   runs integer NOT NULL DEFAULT 0,
                   updated_at timestamptz NOT NULL DEFAULT now(),
                   PRIMARY KEY (org_id, pipeline, step)
                 );",
            )
            .await?;
        Ok(())
    }

    async fn down(&self, m: &SchemaManager) -> Result<(), DbErr> {
        m.get_connection()
            .execute_unprepared("DROP TABLE IF EXISTS pipeline_cursors;")
            .await?;
        Ok(())
    }
}
