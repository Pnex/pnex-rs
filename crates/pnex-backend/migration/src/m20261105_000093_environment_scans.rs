//! Image scans of an environment (agent-environments.md D720): each one is a runner job,
//! whose state it takes, plus the report it brought back.

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, m: &SchemaManager) -> Result<(), DbErr> {
        m.get_connection()
            .execute_unprepared(
                "CREATE TABLE environment_scans (
                   id uuid PRIMARY KEY,
                   org_id bigint NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
                   build_id uuid NOT NULL REFERENCES environment_builds(id) ON DELETE CASCADE,
                   job_id uuid NOT NULL REFERENCES runner_jobs(id) ON DELETE CASCADE,
                   report jsonb,
                   created_at timestamptz NOT NULL DEFAULT now()
                 );
                 CREATE INDEX environment_scans_of ON environment_scans (build_id, created_at);
                 CREATE UNIQUE INDEX environment_scans_job ON environment_scans (job_id);
                 ALTER TABLE runner_jobs DROP CONSTRAINT runner_jobs_kind_check;
                 ALTER TABLE runner_jobs ADD CONSTRAINT runner_jobs_kind_check
                   CHECK (kind IN ('probe', 'environment_build', 'environment_scan'));",
            )
            .await?;
        Ok(())
    }

    async fn down(&self, m: &SchemaManager) -> Result<(), DbErr> {
        m.get_connection()
            .execute_unprepared(
                "DROP TABLE environment_scans;
                 DELETE FROM runner_jobs WHERE kind = 'environment_scan';
                 ALTER TABLE runner_jobs DROP CONSTRAINT runner_jobs_kind_check;
                 ALTER TABLE runner_jobs ADD CONSTRAINT runner_jobs_kind_check
                   CHECK (kind IN ('probe', 'environment_build'));",
            )
            .await?;
        Ok(())
    }
}
