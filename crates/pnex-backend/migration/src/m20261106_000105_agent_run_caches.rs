//! Dependency caches of an agent run (agent-environments.md D730): what the sandbox restores
//! before setup and saves after the agent, by name and key, under the environment.

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, m: &SchemaManager) -> Result<(), DbErr> {
        m.get_connection()
            .execute_unprepared(
                "ALTER TABLE agent_runs ADD COLUMN caches jsonb NOT NULL DEFAULT '[]';",
            )
            .await?;
        Ok(())
    }

    async fn down(&self, m: &SchemaManager) -> Result<(), DbErr> {
        m.get_connection()
            .execute_unprepared("ALTER TABLE agent_runs DROP COLUMN caches;")
            .await?;
        Ok(())
    }
}
