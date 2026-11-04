//! Pipelines (data-pipelines.md D571, D573, D574): versioned documents, their
//! job queue, and the lineage each run writes. `pnex_pipeline_worker` is the
//! role of the worker that runs user SQL: catalogue and queue only, no business
//! table and no secret.

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, m: &SchemaManager) -> Result<(), DbErr> {
        m.get_connection()
            .execute_unprepared(
                "CREATE TABLE pipelines (
                   id uuid PRIMARY KEY DEFAULT gen_random_uuid(),
                   org_id bigint NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
                   name text NOT NULL,
                   version integer NOT NULL DEFAULT 1,
                   document text NOT NULL,
                   created_at timestamptz NOT NULL DEFAULT now(),
                   updated_at timestamptz NOT NULL DEFAULT now(),
                   UNIQUE (org_id, name)
                 );
                 CREATE TABLE pipeline_versions (
                   pipeline_id uuid NOT NULL REFERENCES pipelines(id) ON DELETE CASCADE,
                   version integer NOT NULL,
                   document text NOT NULL,
                   created_at timestamptz NOT NULL DEFAULT now(),
                   PRIMARY KEY (pipeline_id, version)
                 );
                 CREATE TABLE pipeline_jobs (
                   id uuid PRIMARY KEY DEFAULT gen_random_uuid(),
                   org_id bigint NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
                   pipeline_id uuid NOT NULL REFERENCES pipelines(id) ON DELETE CASCADE,
                   version integer NOT NULL,
                   trigger text NOT NULL CHECK (trigger IN ('manual')),
                   state text NOT NULL DEFAULT 'queued'
                     CHECK (state IN ('queued', 'running', 'done', 'failed')),
                   attempts integer NOT NULL DEFAULT 0,
                   code text,
                   args jsonb,
                   heartbeat_at timestamptz,
                   created_at timestamptz NOT NULL DEFAULT now(),
                   started_at timestamptz,
                   finished_at timestamptz
                 );
                 CREATE UNIQUE INDEX pipeline_jobs_one_active
                   ON pipeline_jobs (pipeline_id) WHERE state IN ('queued', 'running');
                 CREATE UNIQUE INDEX pipeline_jobs_one_running_per_org
                   ON pipeline_jobs (org_id) WHERE state = 'running';
                 CREATE INDEX pipeline_jobs_queue
                   ON pipeline_jobs (created_at) WHERE state = 'queued';
                 CREATE INDEX pipeline_jobs_of_pipeline
                   ON pipeline_jobs (pipeline_id, created_at DESC);
                 CREATE TABLE dataset_transaction_inputs (
                   transaction_id uuid NOT NULL
                     REFERENCES dataset_transactions(id) ON DELETE CASCADE,
                   input_transaction_id uuid NOT NULL
                     REFERENCES dataset_transactions(id) ON DELETE CASCADE,
                   pipeline_id uuid NOT NULL REFERENCES pipelines(id) ON DELETE CASCADE,
                   pipeline_version integer NOT NULL,
                   PRIMARY KEY (transaction_id, input_transaction_id)
                 );
                 CREATE INDEX dataset_transaction_inputs_input
                   ON dataset_transaction_inputs (input_transaction_id);
                 DO $$ BEGIN
                   IF NOT EXISTS (SELECT 1 FROM pg_roles WHERE rolname = 'pnex_pipeline_worker') THEN
                     CREATE ROLE pnex_pipeline_worker NOLOGIN;
                   END IF;
                 END $$;
                 GRANT SELECT ON pipelines, pipeline_versions TO pnex_pipeline_worker;
                 GRANT SELECT, UPDATE ON pipeline_jobs TO pnex_pipeline_worker;
                 GRANT SELECT, INSERT, UPDATE ON datasets, dataset_transactions TO pnex_pipeline_worker;
                 GRANT SELECT, INSERT ON dataset_files, dataset_transaction_inputs TO pnex_pipeline_worker;
                 GRANT USAGE ON SEQUENCE dataset_commit_seq TO pnex_pipeline_worker;",
            )
            .await?;
        Ok(())
    }

    async fn down(&self, m: &SchemaManager) -> Result<(), DbErr> {
        m.get_connection()
            .execute_unprepared(
                "REVOKE ALL ON pipelines, pipeline_versions, pipeline_jobs, datasets,
                   dataset_transactions, dataset_files, dataset_transaction_inputs
                   FROM pnex_pipeline_worker;
                 REVOKE ALL ON SEQUENCE dataset_commit_seq FROM pnex_pipeline_worker;
                 DROP TABLE dataset_transaction_inputs;
                 DROP TABLE pipeline_jobs;
                 DROP TABLE pipeline_versions;
                 DROP TABLE pipelines;",
            )
            .await?;
        Ok(())
    }
}
