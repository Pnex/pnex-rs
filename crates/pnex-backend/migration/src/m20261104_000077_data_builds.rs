//! Chained runs and commit checks (data-pipelines.md D575, D576, D578): a build runs
//! a target and its dependencies in order, a transaction keeps its check results,
//! a dataset can require a new version every so often, and committed versions wait
//! in place until the server publishes them.

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, m: &SchemaManager) -> Result<(), DbErr> {
        m.get_connection()
            .execute_unprepared(
                "CREATE TABLE data_builds (
                   id uuid PRIMARY KEY DEFAULT gen_random_uuid(),
                   org_id bigint NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
                   target_kind text NOT NULL CHECK (target_kind IN ('pipeline', 'import')),
                   target_id uuid NOT NULL,
                   target_name text NOT NULL,
                   trigger text NOT NULL CHECK (trigger IN ('schedule', 'manual')),
                   requested_by bigint REFERENCES users(id) ON DELETE SET NULL,
                   state text NOT NULL DEFAULT 'running'
                     CHECK (state IN ('running', 'done', 'failed')),
                   again boolean NOT NULL DEFAULT false,
                   code text,
                   args jsonb,
                   steps jsonb NOT NULL DEFAULT '[]',
                   step_started_at timestamptz,
                   created_at timestamptz NOT NULL DEFAULT now(),
                   finished_at timestamptz
                 );
                 CREATE UNIQUE INDEX data_builds_one_running
                   ON data_builds (target_kind, target_id) WHERE state = 'running';
                 CREATE INDEX data_builds_running ON data_builds (created_at)
                   WHERE state = 'running';
                 CREATE INDEX data_builds_of_org ON data_builds (org_id, created_at DESC);
                 ALTER TABLE pipelines
                   ADD COLUMN recurrence jsonb,
                   ADD COLUMN next_run_at timestamptz,
                   ADD COLUMN scheduled_by bigint REFERENCES users(id) ON DELETE SET NULL;
                 CREATE INDEX pipelines_due ON pipelines (next_run_at)
                   WHERE next_run_at IS NOT NULL;
                 ALTER TABLE pipeline_jobs
                   DROP CONSTRAINT pipeline_jobs_trigger_check,
                   ADD CONSTRAINT pipeline_jobs_trigger_check
                     CHECK (trigger IN ('manual', 'erasure', 'build')),
                   ADD COLUMN notified_at timestamptz;
                 UPDATE pipeline_jobs SET notified_at = now() WHERE state = 'failed';
                 CREATE INDEX pipeline_jobs_to_notify ON pipeline_jobs (finished_at)
                   WHERE state = 'failed' AND notified_at IS NULL;
                 ALTER TABLE dataset_transactions
                   ADD COLUMN checks jsonb NOT NULL DEFAULT '[]',
                   ADD COLUMN published_at timestamptz;
                 UPDATE dataset_transactions SET published_at = now() WHERE state = 'committed';
                 CREATE INDEX dataset_transactions_to_publish
                   ON dataset_transactions (seq)
                   WHERE state = 'committed' AND published_at IS NULL;
                 ALTER TABLE object_import_runs ADD COLUMN published_at timestamptz;
                 UPDATE object_import_runs SET published_at = now() WHERE state = 'applied';
                 CREATE INDEX object_import_runs_to_publish
                   ON object_import_runs (finished_at)
                   WHERE state = 'applied' AND published_at IS NULL;
                 ALTER TABLE datasets
                   ADD COLUMN max_age_hours integer,
                   ADD COLUMN stale_notified_at timestamptz;",
            )
            .await?;
        Ok(())
    }

    async fn down(&self, m: &SchemaManager) -> Result<(), DbErr> {
        m.get_connection()
            .execute_unprepared(
                "ALTER TABLE datasets DROP COLUMN stale_notified_at, DROP COLUMN max_age_hours;
                 DROP INDEX object_import_runs_to_publish;
                 ALTER TABLE object_import_runs DROP COLUMN published_at;
                 DROP INDEX dataset_transactions_to_publish;
                 ALTER TABLE dataset_transactions DROP COLUMN published_at, DROP COLUMN checks;
                 DROP INDEX pipeline_jobs_to_notify;
                 DELETE FROM pipeline_jobs WHERE trigger = 'build';
                 ALTER TABLE pipeline_jobs
                   DROP COLUMN notified_at,
                   DROP CONSTRAINT pipeline_jobs_trigger_check,
                   ADD CONSTRAINT pipeline_jobs_trigger_check
                     CHECK (trigger IN ('manual', 'erasure'));
                 DROP INDEX pipelines_due;
                 ALTER TABLE pipelines
                   DROP COLUMN scheduled_by, DROP COLUMN next_run_at, DROP COLUMN recurrence;
                 DROP TABLE data_builds;",
            )
            .await?;
        Ok(())
    }
}
