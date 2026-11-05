//! Runs of hosted agents (agent-environments.md D731): one sandbox job each, a run token
//! stored hashed, and the usage counted by the LLM gateway (D732).

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, m: &SchemaManager) -> Result<(), DbErr> {
        m.get_connection()
            .execute_unprepared(
                "CREATE TABLE agent_runs (
                   id uuid PRIMARY KEY,
                   org_id bigint NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
                   environment_id uuid NOT NULL REFERENCES environments(id) ON DELETE CASCADE,
                   build_id uuid NOT NULL REFERENCES environment_builds(id) ON DELETE CASCADE,
                   job_id uuid NOT NULL REFERENCES runner_jobs(id) ON DELETE CASCADE,
                   env jsonb NOT NULL DEFAULT '{}',
                   token_hash text,
                   llm_calls integer NOT NULL DEFAULT 0,
                   llm_calls_max integer NOT NULL CHECK (llm_calls_max > 0),
                   input_tokens bigint NOT NULL DEFAULT 0,
                   output_tokens bigint NOT NULL DEFAULT 0,
                   created_by bigint REFERENCES users(id) ON DELETE SET NULL,
                   created_at timestamptz NOT NULL DEFAULT now()
                 );
                 CREATE INDEX agent_runs_of_org ON agent_runs (org_id, created_at);
                 CREATE UNIQUE INDEX agent_runs_job ON agent_runs (job_id);
                 CREATE UNIQUE INDEX agent_runs_token ON agent_runs (token_hash);
                 ALTER TABLE runner_jobs DROP CONSTRAINT runner_jobs_kind_check;
                 ALTER TABLE runner_jobs ADD CONSTRAINT runner_jobs_kind_check
                   CHECK (kind IN ('probe', 'environment_build', 'environment_scan', 'sandbox'));",
            )
            .await?;
        Ok(())
    }

    async fn down(&self, m: &SchemaManager) -> Result<(), DbErr> {
        m.get_connection()
            .execute_unprepared(
                "DROP TABLE agent_runs;
                 DELETE FROM runner_jobs WHERE kind = 'sandbox';
                 ALTER TABLE runner_jobs DROP CONSTRAINT runner_jobs_kind_check;
                 ALTER TABLE runner_jobs ADD CONSTRAINT runner_jobs_kind_check
                   CHECK (kind IN ('probe', 'environment_build', 'environment_scan'));",
            )
            .await?;
        Ok(())
    }
}
