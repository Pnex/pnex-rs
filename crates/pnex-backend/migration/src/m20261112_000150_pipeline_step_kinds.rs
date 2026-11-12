//! A queued pipeline step is a query or a Starlark `call` (pipeline-engine.md
//! D936): `sql` holds its text, `settings` the pack settings a call reads.

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, m: &SchemaManager) -> Result<(), DbErr> {
        m.get_connection()
            .execute_unprepared(
                "ALTER TABLE pipeline_step_jobs
                   ADD COLUMN kind text NOT NULL DEFAULT 'sql' CHECK (kind IN ('sql', 'call')),
                   ADD COLUMN settings jsonb NOT NULL DEFAULT '{}';",
            )
            .await?;
        Ok(())
    }

    async fn down(&self, m: &SchemaManager) -> Result<(), DbErr> {
        m.get_connection()
            .execute_unprepared(
                "ALTER TABLE pipeline_step_jobs DROP COLUMN kind, DROP COLUMN settings;",
            )
            .await?;
        Ok(())
    }
}
