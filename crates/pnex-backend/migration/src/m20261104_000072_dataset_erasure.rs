//! Erasure in datasets (data-pipelines.md D569): which columns identify which
//! objects, and the durable queue of erasures still to apply to the files. The
//! worker sees only that an org has one pending, never the erased values.

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, m: &SchemaManager) -> Result<(), DbErr> {
        m.get_connection()
            .execute_unprepared(
                "CREATE TABLE dataset_identity_columns (
                   dataset_id uuid NOT NULL REFERENCES datasets(id) ON DELETE CASCADE,
                   column_name text NOT NULL,
                   type_key text NOT NULL,
                   property text NOT NULL,
                   PRIMARY KEY (dataset_id, column_name, type_key)
                 );
                 CREATE TABLE dataset_erasures (
                   id uuid PRIMARY KEY DEFAULT gen_random_uuid(),
                   org_id bigint NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
                   dataset_id uuid NOT NULL REFERENCES datasets(id) ON DELETE CASCADE,
                   column_name text NOT NULL,
                   value text NOT NULL,
                   type_key text NOT NULL,
                   hit boolean NOT NULL DEFAULT false,
                   created_at timestamptz NOT NULL DEFAULT now()
                 );
                 CREATE INDEX dataset_erasures_org ON dataset_erasures (org_id);
                 GRANT SELECT (org_id) ON dataset_erasures TO pnex_pipeline_worker;
                 ALTER TABLE pipeline_jobs DROP CONSTRAINT pipeline_jobs_trigger_check;
                 ALTER TABLE pipeline_jobs ADD CONSTRAINT pipeline_jobs_trigger_check
                   CHECK (trigger IN ('manual', 'erasure'));",
            )
            .await?;
        Ok(())
    }

    async fn down(&self, m: &SchemaManager) -> Result<(), DbErr> {
        m.get_connection()
            .execute_unprepared(
                "DELETE FROM pipeline_jobs WHERE trigger = 'erasure';
                 ALTER TABLE pipeline_jobs DROP CONSTRAINT pipeline_jobs_trigger_check;
                 ALTER TABLE pipeline_jobs ADD CONSTRAINT pipeline_jobs_trigger_check
                   CHECK (trigger IN ('manual'));
                 REVOKE ALL ON dataset_erasures FROM pnex_pipeline_worker;
                 DROP TABLE dataset_erasures;
                 DROP TABLE dataset_identity_columns;",
            )
            .await?;
        Ok(())
    }
}
