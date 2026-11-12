//! The `sql` steps of pipelines (pipeline-engine.md D936, D943): the server
//! stages the working tables a query reads as Parquet and queues it; the
//! worker runs it with the transforms' guards and writes the result back
//! under `output_key`. A job is kept a day.

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, m: &SchemaManager) -> Result<(), DbErr> {
        m.get_connection()
            .execute_unprepared(
                "CREATE TABLE pipeline_step_jobs (
                   id uuid PRIMARY KEY,
                   org_id bigint NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
                   sql text NOT NULL,
                   -- SQL name -> object store key of a staged working table.
                   staged jsonb NOT NULL DEFAULT '{}',
                   -- SQL name -> name of a table of the org.
                   tables jsonb NOT NULL DEFAULT '{}',
                   output_key text NOT NULL,
                   state text NOT NULL DEFAULT 'queued'
                     CHECK (state IN ('queued', 'running', 'done', 'failed')),
                   rows bigint,
                   code text,
                   args jsonb,
                   attempts integer NOT NULL DEFAULT 0,
                   created_at timestamptz NOT NULL DEFAULT now(),
                   started_at timestamptz,
                   finished_at timestamptz
                 );
                 CREATE INDEX pipeline_step_jobs_queued ON pipeline_step_jobs (created_at)
                   WHERE state = 'queued';
                 DO $$ BEGIN
                   IF EXISTS (SELECT 1 FROM pg_roles WHERE rolname = 'pnex_pipeline_worker') THEN
                     GRANT SELECT, UPDATE ON pipeline_step_jobs TO pnex_pipeline_worker;
                   END IF;
                 END $$;",
            )
            .await?;
        Ok(())
    }

    async fn down(&self, m: &SchemaManager) -> Result<(), DbErr> {
        m.get_connection()
            .execute_unprepared("DROP TABLE pipeline_step_jobs;")
            .await?;
        Ok(())
    }
}
