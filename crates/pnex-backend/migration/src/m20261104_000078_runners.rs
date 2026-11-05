//! Runners of the worker fabric (agent-environments.md lot E0): single-use join
//! tokens, enrolled runners, and the queue of jobs they poll for. Tokens are stored
//! hashed; a job only reaches a runner of its own org.

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, m: &SchemaManager) -> Result<(), DbErr> {
        m.get_connection()
            .execute_unprepared(
                "CREATE TABLE runner_join_tokens (
                   id uuid PRIMARY KEY DEFAULT gen_random_uuid(),
                   org_id bigint NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
                   name text NOT NULL,
                   token_hash text NOT NULL UNIQUE,
                   created_by bigint REFERENCES users(id) ON DELETE SET NULL,
                   expires_at timestamptz NOT NULL,
                   used_at timestamptz,
                   created_at timestamptz NOT NULL DEFAULT now()
                 );
                 CREATE TABLE runners (
                   id uuid PRIMARY KEY DEFAULT gen_random_uuid(),
                   org_id bigint NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
                   name text NOT NULL,
                   token_hash text NOT NULL UNIQUE,
                   capabilities jsonb NOT NULL DEFAULT '[]',
                   last_seen_at timestamptz,
                   created_at timestamptz NOT NULL DEFAULT now()
                 );
                 CREATE INDEX runners_of_org ON runners (org_id, created_at DESC);
                 CREATE TABLE runner_jobs (
                   id uuid PRIMARY KEY DEFAULT gen_random_uuid(),
                   org_id bigint NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
                   kind text NOT NULL CHECK (kind IN ('probe')),
                   spec jsonb NOT NULL,
                   required_capabilities jsonb NOT NULL DEFAULT '[]',
                   state text NOT NULL DEFAULT 'queued'
                     CHECK (state IN ('queued', 'running', 'succeeded', 'failed', 'expired')),
                   runner_id uuid REFERENCES runners(id) ON DELETE SET NULL,
                   attempts integer NOT NULL DEFAULT 0,
                   outcome text,
                   exit_code integer,
                   lease_expires_at timestamptz,
                   queue_expires_at timestamptz NOT NULL,
                   created_by bigint REFERENCES users(id) ON DELETE SET NULL,
                   created_at timestamptz NOT NULL DEFAULT now(),
                   started_at timestamptz,
                   finished_at timestamptz
                 );
                 CREATE INDEX runner_jobs_queued ON runner_jobs (org_id, created_at)
                   WHERE state = 'queued';
                 CREATE INDEX runner_jobs_running ON runner_jobs (lease_expires_at)
                   WHERE state = 'running';
                 CREATE INDEX runner_jobs_of_org ON runner_jobs (org_id, created_at DESC);",
            )
            .await?;
        Ok(())
    }

    async fn down(&self, m: &SchemaManager) -> Result<(), DbErr> {
        m.get_connection()
            .execute_unprepared(
                "DROP TABLE IF EXISTS runner_jobs;
                 DROP TABLE IF EXISTS runners;
                 DROP TABLE IF EXISTS runner_join_tokens;",
            )
            .await?;
        Ok(())
    }
}
