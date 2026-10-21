//! Flow import runs outlive the HTTP call of their step (ingestion.md
//! D322): `heartbeat_at` is refreshed while a run works, so a run whose
//! pod died is told from one still running on another pod; the ingest
//! pass and node that started a run find it again when they poll.

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, m: &SchemaManager) -> Result<(), DbErr> {
        m.get_connection()
            .execute_unprepared(
                "ALTER TABLE object_import_runs
                    ADD COLUMN heartbeat_at timestamptz NOT NULL DEFAULT now(),
                    ADD COLUMN ingest_run_id uuid,
                    ADD COLUMN ingest_node_id varchar(64);
                CREATE UNIQUE INDEX uniq_object_import_runs_ingest
                    ON object_import_runs (org_id, ingest_run_id, ingest_node_id)
                    WHERE ingest_run_id IS NOT NULL;",
            )
            .await?;
        Ok(())
    }

    async fn down(&self, m: &SchemaManager) -> Result<(), DbErr> {
        m.get_connection()
            .execute_unprepared(
                "DROP INDEX IF EXISTS uniq_object_import_runs_ingest;
                ALTER TABLE object_import_runs
                    DROP COLUMN IF EXISTS heartbeat_at,
                    DROP COLUMN IF EXISTS ingest_run_id,
                    DROP COLUMN IF EXISTS ingest_node_id;",
            )
            .await?;
        Ok(())
    }
}
