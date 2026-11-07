//! Run engines (D781): server browser, internal LLM and extension jobs live in
//! `runner_jobs` beside the runner kinds; their runs have no environment nor build.

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, m: &SchemaManager) -> Result<(), DbErr> {
        m.get_connection()
            .execute_unprepared(
                "ALTER TABLE runner_jobs DROP CONSTRAINT runner_jobs_kind_check;
                 ALTER TABLE runner_jobs ADD CONSTRAINT runner_jobs_kind_check
                   CHECK (kind IN ('probe', 'environment_build', 'environment_scan', 'sandbox',
                                   'server_browser', 'internal_llm', 'extension'));
                 ALTER TABLE agent_runs ALTER COLUMN environment_id DROP NOT NULL;
                 ALTER TABLE agent_runs ALTER COLUMN build_id DROP NOT NULL;",
            )
            .await?;
        Ok(())
    }

    async fn down(&self, m: &SchemaManager) -> Result<(), DbErr> {
        m.get_connection()
            .execute_unprepared(
                "DELETE FROM runner_jobs WHERE kind IN ('server_browser', 'internal_llm', 'extension');
                 DELETE FROM agent_runs WHERE environment_id IS NULL OR build_id IS NULL;
                 ALTER TABLE agent_runs ALTER COLUMN environment_id SET NOT NULL;
                 ALTER TABLE agent_runs ALTER COLUMN build_id SET NOT NULL;
                 ALTER TABLE runner_jobs DROP CONSTRAINT runner_jobs_kind_check;
                 ALTER TABLE runner_jobs ADD CONSTRAINT runner_jobs_kind_check
                   CHECK (kind IN ('probe', 'environment_build', 'environment_scan', 'sandbox'));",
            )
            .await?;
        Ok(())
    }
}
