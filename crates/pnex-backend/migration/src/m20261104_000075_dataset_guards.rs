//! Guards on datasets (data-pipelines.md D569, D577): the table an import fills
//! belongs to it, an erasure stays as a fingerprint that later loads filter, and
//! lineage outlives a deleted pipeline so erasure still follows it.

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, m: &SchemaManager) -> Result<(), DbErr> {
        m.get_connection()
            .execute_unprepared(
                "ALTER TABLE datasets ADD COLUMN import_id uuid
                   REFERENCES object_imports(id) ON DELETE SET NULL;
                 ALTER TABLE dataset_erasures ADD COLUMN done_at timestamptz;
                 CREATE INDEX dataset_erasures_pending ON dataset_erasures (org_id)
                   WHERE done_at IS NULL;
                 DO $$ BEGIN
                   IF EXISTS (SELECT 1 FROM pg_roles WHERE rolname = 'pnex_pipeline_worker') THEN
                     GRANT SELECT (done_at) ON dataset_erasures TO pnex_pipeline_worker;
                   END IF;
                 END $$;
                 ALTER TABLE dataset_transaction_inputs
                   ALTER COLUMN pipeline_id DROP NOT NULL,
                   DROP CONSTRAINT dataset_transaction_inputs_pipeline_id_fkey,
                   ADD CONSTRAINT dataset_transaction_inputs_pipeline_id_fkey
                     FOREIGN KEY (pipeline_id) REFERENCES pipelines(id) ON DELETE SET NULL;",
            )
            .await?;
        Ok(())
    }

    async fn down(&self, m: &SchemaManager) -> Result<(), DbErr> {
        m.get_connection()
            .execute_unprepared(
                "DELETE FROM dataset_transaction_inputs WHERE pipeline_id IS NULL;
                 ALTER TABLE dataset_transaction_inputs
                   ALTER COLUMN pipeline_id SET NOT NULL,
                   DROP CONSTRAINT dataset_transaction_inputs_pipeline_id_fkey,
                   ADD CONSTRAINT dataset_transaction_inputs_pipeline_id_fkey
                     FOREIGN KEY (pipeline_id) REFERENCES pipelines(id) ON DELETE CASCADE;
                 DO $$ BEGIN
                   IF EXISTS (SELECT 1 FROM pg_roles WHERE rolname = 'pnex_pipeline_worker') THEN
                     REVOKE SELECT (done_at) ON dataset_erasures FROM pnex_pipeline_worker;
                   END IF;
                 END $$;
                 DROP INDEX dataset_erasures_pending;
                 ALTER TABLE dataset_erasures DROP COLUMN done_at;
                 ALTER TABLE datasets DROP COLUMN import_id;",
            )
            .await?;
        Ok(())
    }
}
