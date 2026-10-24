//! The flow node `ingest` is retired (web-presence.md D359): the columns
//! tying an import run to an ingestion pass and node go, with their index.

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, m: &SchemaManager) -> Result<(), DbErr> {
        m.get_connection()
            .execute_unprepared(
                "ALTER TABLE object_import_runs \
                    DROP COLUMN IF EXISTS ingest_run_id, \
                    DROP COLUMN IF EXISTS ingest_node_id;",
            )
            .await?;
        Ok(())
    }

    async fn down(&self, m: &SchemaManager) -> Result<(), DbErr> {
        m.get_connection()
            .execute_unprepared(
                "ALTER TABLE object_import_runs \
                    ADD COLUMN ingest_run_id uuid, \
                    ADD COLUMN ingest_node_id varchar(64); \
                 CREATE UNIQUE INDEX IF NOT EXISTS uniq_object_import_runs_ingest \
                    ON object_import_runs (org_id, ingest_run_id, ingest_node_id) \
                    WHERE ingest_run_id IS NOT NULL;",
            )
            .await?;
        Ok(())
    }
}
