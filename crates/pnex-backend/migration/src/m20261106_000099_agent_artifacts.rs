//! Artifacts of agent runs (agent-environments.md D729, D744): files kept in the store of
//! the org between runs, a row each with its checksum and expiry, never the content. A run
//! keeps what it declared and its own artifact token, stored hashed like its run token.

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, m: &SchemaManager) -> Result<(), DbErr> {
        m.get_connection()
            .execute_unprepared(
                "CREATE TABLE agent_artifacts (
                   id uuid PRIMARY KEY,
                   org_id bigint NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
                   run_id uuid REFERENCES agent_runs(id) ON DELETE CASCADE,
                   name text NOT NULL,
                   state text NOT NULL CHECK (state IN ('pending', 'ready', 'refused', 'missing')),
                   form text CHECK (form IN ('file', 'tar')),
                   error text,
                   sha256 text,
                   size bigint,
                   storage_key text NOT NULL,
                   created_by bigint REFERENCES users(id) ON DELETE SET NULL,
                   created_at timestamptz NOT NULL DEFAULT now(),
                   expires_at timestamptz NOT NULL
                 );
                 CREATE INDEX agent_artifacts_of_org ON agent_artifacts (org_id, created_at);
                 CREATE UNIQUE INDEX agent_artifacts_output ON agent_artifacts (run_id, name)
                   WHERE run_id IS NOT NULL;
                 CREATE INDEX agent_artifacts_expiry ON agent_artifacts (expires_at);
                 ALTER TABLE agent_runs ADD COLUMN inputs jsonb NOT NULL DEFAULT '[]';
                 ALTER TABLE agent_runs ADD COLUMN outputs jsonb NOT NULL DEFAULT '[]';
                 ALTER TABLE agent_runs ADD COLUMN artifact_token_hash text;
                 ALTER TABLE agent_runs ADD COLUMN error text;
                 CREATE UNIQUE INDEX agent_runs_artifact_token ON agent_runs (artifact_token_hash);",
            )
            .await?;
        Ok(())
    }

    async fn down(&self, m: &SchemaManager) -> Result<(), DbErr> {
        m.get_connection()
            .execute_unprepared(
                "DROP TABLE agent_artifacts;
                 DROP INDEX agent_runs_artifact_token;
                 ALTER TABLE agent_runs DROP COLUMN inputs, DROP COLUMN outputs,
                   DROP COLUMN artifact_token_hash, DROP COLUMN error;",
            )
            .await?;
        Ok(())
    }
}
