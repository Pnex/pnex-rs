//! Builds of an agent environment (agent-environments.md D700, D701): each one is a
//! runner job, whose state it takes, plus the digest of the image it pushed.

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, m: &SchemaManager) -> Result<(), DbErr> {
        m.get_connection()
            .execute_unprepared(
                "CREATE TABLE environment_builds (
                   id uuid PRIMARY KEY,
                   org_id bigint NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
                   environment_id uuid NOT NULL,
                   number integer NOT NULL,
                   job_id uuid NOT NULL REFERENCES runner_jobs(id) ON DELETE CASCADE,
                   repository text NOT NULL,
                   digest text,
                   created_by bigint REFERENCES users(id) ON DELETE SET NULL,
                   created_at timestamptz NOT NULL DEFAULT now(),
                   FOREIGN KEY (environment_id, number)
                     REFERENCES environment_versions(environment_id, number) ON DELETE CASCADE
                 );
                 CREATE INDEX environment_builds_of ON environment_builds (environment_id, created_at);
                 CREATE UNIQUE INDEX environment_builds_job ON environment_builds (job_id);
                 ALTER TABLE runner_jobs DROP CONSTRAINT runner_jobs_kind_check;
                 ALTER TABLE runner_jobs ADD CONSTRAINT runner_jobs_kind_check
                   CHECK (kind IN ('probe', 'environment_build'));",
            )
            .await?;
        Ok(())
    }

    async fn down(&self, m: &SchemaManager) -> Result<(), DbErr> {
        m.get_connection()
            .execute_unprepared(
                "DROP TABLE environment_builds;
                 DELETE FROM runner_jobs WHERE kind = 'environment_build';
                 ALTER TABLE runner_jobs DROP CONSTRAINT runner_jobs_kind_check;
                 ALTER TABLE runner_jobs ADD CONSTRAINT runner_jobs_kind_check
                   CHECK (kind IN ('probe'));",
            )
            .await?;
        Ok(())
    }
}
