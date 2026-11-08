//! Read-only SQL queries (analysis.md D825–D827): what the server fixed for each
//! query, run by the worker before pipeline runs, kept a day with their result.

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, m: &SchemaManager) -> Result<(), DbErr> {
        m.get_connection()
            .execute_unprepared(
                "CREATE TABLE analysis_queries (
                   id uuid PRIMARY KEY,
                   org_id bigint NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
                   user_id bigint NOT NULL REFERENCES users(id) ON DELETE CASCADE,
                   sql text NOT NULL,
                   state text NOT NULL DEFAULT 'queued'
                     CHECK (state IN ('queued', 'running', 'done', 'failed')),
                   -- name -> dataset id the caller may read, names refused by a marking,
                   -- object type -> object store key of its frozen view.
                   tables jsonb NOT NULL DEFAULT '{}',
                   refused jsonb NOT NULL DEFAULT '[]',
                   objects jsonb NOT NULL DEFAULT '{}',
                   result jsonb,
                   code text,
                   args jsonb,
                   attempts integer NOT NULL DEFAULT 0,
                   created_at timestamptz NOT NULL DEFAULT now(),
                   started_at timestamptz,
                   finished_at timestamptz
                 );
                 CREATE INDEX analysis_queries_queued ON analysis_queries (created_at)
                   WHERE state = 'queued';
                 -- One query at a time per person (D826).
                 CREATE UNIQUE INDEX analysis_queries_one_per_user ON analysis_queries (user_id)
                   WHERE state IN ('queued', 'running');
                 DO $$ BEGIN
                   IF EXISTS (SELECT 1 FROM pg_roles WHERE rolname = 'pnex_pipeline_worker') THEN
                     GRANT SELECT, UPDATE ON analysis_queries TO pnex_pipeline_worker;
                   END IF;
                 END $$;",
            )
            .await?;
        Ok(())
    }

    async fn down(&self, m: &SchemaManager) -> Result<(), DbErr> {
        m.get_connection()
            .execute_unprepared("DROP TABLE analysis_queries;")
            .await?;
        Ok(())
    }
}
