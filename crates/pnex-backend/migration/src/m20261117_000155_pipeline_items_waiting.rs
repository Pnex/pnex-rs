//! A pipeline row may wait for its navigation (pipeline-engine.md D995).

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, m: &SchemaManager) -> Result<(), DbErr> {
        m.get_connection()
            .execute_unprepared(
                "ALTER TABLE pipeline_items DROP CONSTRAINT pipeline_items_state_check,
                   ADD CONSTRAINT pipeline_items_state_check
                     CHECK (state IN ('done', 'empty', 'failed', 'waiting'));",
            )
            .await?;
        Ok(())
    }

    async fn down(&self, m: &SchemaManager) -> Result<(), DbErr> {
        m.get_connection()
            .execute_unprepared(
                "DELETE FROM pipeline_items WHERE state = 'waiting';
                 ALTER TABLE pipeline_items DROP CONSTRAINT pipeline_items_state_check,
                   ADD CONSTRAINT pipeline_items_state_check
                     CHECK (state IN ('done', 'empty', 'failed'));",
            )
            .await?;
        Ok(())
    }
}
