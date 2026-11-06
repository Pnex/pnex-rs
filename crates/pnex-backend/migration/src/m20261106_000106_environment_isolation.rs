//! Isolation of the runs of an environment (agent-environments.md D763): a pod by default,
//! a micro-VM on request.

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, m: &SchemaManager) -> Result<(), DbErr> {
        m.get_connection()
            .execute_unprepared(
                "ALTER TABLE environments ADD COLUMN isolation text NOT NULL DEFAULT 'pod' \
                 CHECK (isolation IN ('pod', 'vm'));",
            )
            .await?;
        Ok(())
    }

    async fn down(&self, m: &SchemaManager) -> Result<(), DbErr> {
        m.get_connection()
            .execute_unprepared("ALTER TABLE environments DROP COLUMN isolation;")
            .await?;
        Ok(())
    }
}
