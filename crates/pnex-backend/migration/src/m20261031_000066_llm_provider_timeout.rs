//! Per-provider timeout of one LLM call: a CPU-only local model needs
//! minutes where a hosted API answers in seconds.

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, m: &SchemaManager) -> Result<(), DbErr> {
        m.get_connection()
            .execute_unprepared(
                r"ALTER TABLE llm_providers
                    ADD COLUMN timeout_secs integer NOT NULL DEFAULT 90
                        CHECK (timeout_secs BETWEEN 10 AND 900);",
            )
            .await?;
        Ok(())
    }

    async fn down(&self, m: &SchemaManager) -> Result<(), DbErr> {
        m.get_connection()
            .execute_unprepared("ALTER TABLE llm_providers DROP COLUMN IF EXISTS timeout_secs;")
            .await?;
        Ok(())
    }
}
