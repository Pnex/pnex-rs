//! Validation gate of agents (validation-gate.md D839): the circuit breaker of a validator
//! agent. The state of each request lives in its `tool_call` proposal.

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, m: &SchemaManager) -> Result<(), DbErr> {
        m.get_connection()
            .execute_unprepared(
                "ALTER TABLE agents
                   ADD COLUMN validator_tripped_at timestamptz,
                   -- `rate` or `all_accepted`.
                   ADD COLUMN validator_tripped_reason text,
                   -- Decisions before it no longer count towards `all_accepted`.
                   ADD COLUMN validator_rearmed_at timestamptz;
                 CREATE INDEX proposals_validator_idx ON proposals ((vals->'validator'->>'agent_id'))
                   WHERE kind = 'tool_call';",
            )
            .await?;
        Ok(())
    }

    async fn down(&self, m: &SchemaManager) -> Result<(), DbErr> {
        m.get_connection()
            .execute_unprepared(
                "DROP INDEX IF EXISTS proposals_validator_idx;
                 ALTER TABLE agents DROP COLUMN validator_tripped_at,
                   DROP COLUMN validator_tripped_reason, DROP COLUMN validator_rearmed_at;",
            )
            .await?;
        Ok(())
    }
}
