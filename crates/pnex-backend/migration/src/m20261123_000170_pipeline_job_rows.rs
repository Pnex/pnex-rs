//! The rows a manual run gives to its pipeline's `asked` axis (pipelines.md
//! D996): a site rule run from the Pipelines route carries its parameters.

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, m: &SchemaManager) -> Result<(), DbErr> {
        m.get_connection()
            .execute_unprepared("ALTER TABLE pipeline_jobs ADD COLUMN IF NOT EXISTS asked jsonb;")
            .await?;
        Ok(())
    }

    async fn down(&self, m: &SchemaManager) -> Result<(), DbErr> {
        m.get_connection()
            .execute_unprepared("ALTER TABLE pipeline_jobs DROP COLUMN IF EXISTS asked;")
            .await?;
        Ok(())
    }
}
