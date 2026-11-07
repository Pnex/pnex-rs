//! Merge requests opened for a run's branches (D388): the forge URLs, kept with the run.

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, m: &SchemaManager) -> Result<(), DbErr> {
        m.get_connection()
            .execute_unprepared(
                "ALTER TABLE agent_runs ADD COLUMN merge_requests jsonb NOT NULL DEFAULT '[]'::jsonb",
            )
            .await?;
        Ok(())
    }

    async fn down(&self, m: &SchemaManager) -> Result<(), DbErr> {
        m.get_connection()
            .execute_unprepared("ALTER TABLE agent_runs DROP COLUMN merge_requests")
            .await?;
        Ok(())
    }
}
