//! Chains of agent runs (agent-environments.md D716, D718): one budget and one subject per
//! chain, at most one active chain per subject; the run carries its chain and whether its
//! end was published (D398). A flow remembers who deployed it: its `agent_run` nodes act
//! within that person's rights (D757).

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, m: &SchemaManager) -> Result<(), DbErr> {
        m.get_connection()
            .execute_unprepared(
                "CREATE TABLE agent_chains (
                   id uuid PRIMARY KEY,
                   org_id bigint NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
                   subject text,
                   state varchar(16) NOT NULL DEFAULT 'active' CHECK (state IN ('active', 'closed')),
                   runs_max integer NOT NULL CHECK (runs_max > 0),
                   llm_calls_max integer NOT NULL CHECK (llm_calls_max > 0),
                   rounds_max integer NOT NULL CHECK (rounds_max > 0),
                   created_by bigint REFERENCES users(id) ON DELETE SET NULL,
                   created_at timestamptz NOT NULL DEFAULT now(),
                   closed_at timestamptz
                 );
                 CREATE UNIQUE INDEX agent_chains_one_active_subject ON agent_chains (org_id, subject)
                   WHERE state = 'active' AND subject IS NOT NULL;
                 ALTER TABLE agent_runs
                   ADD COLUMN chain_id uuid REFERENCES agent_chains(id) ON DELETE SET NULL,
                   ADD COLUMN finished_event_at timestamptz;
                 CREATE INDEX agent_runs_of_chain ON agent_runs (chain_id) WHERE chain_id IS NOT NULL;
                 ALTER TABLE flows ADD COLUMN deployed_by bigint REFERENCES users(id) ON DELETE SET NULL;",
            )
            .await?;
        Ok(())
    }

    async fn down(&self, m: &SchemaManager) -> Result<(), DbErr> {
        m.get_connection()
            .execute_unprepared(
                "ALTER TABLE flows DROP COLUMN deployed_by;
                 ALTER TABLE agent_runs DROP COLUMN finished_event_at, DROP COLUMN chain_id;
                 DROP TABLE agent_chains;",
            )
            .await?;
        Ok(())
    }
}
