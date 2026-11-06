//! Repositories and test services of an agent run (agent-environments.md D758): what the
//! sandbox clones and starts beside the agent. A repository names its token in the vault,
//! never the token itself.

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, m: &SchemaManager) -> Result<(), DbErr> {
        m.get_connection()
            .execute_unprepared(
                "ALTER TABLE agent_runs ADD COLUMN repos jsonb NOT NULL DEFAULT '[]';
                 ALTER TABLE agent_runs ADD COLUMN services jsonb NOT NULL DEFAULT '[]';",
            )
            .await?;
        Ok(())
    }

    async fn down(&self, m: &SchemaManager) -> Result<(), DbErr> {
        m.get_connection()
            .execute_unprepared("ALTER TABLE agent_runs DROP COLUMN repos, DROP COLUMN services;")
            .await?;
        Ok(())
    }
}
