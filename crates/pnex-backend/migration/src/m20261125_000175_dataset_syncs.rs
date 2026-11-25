//! A run whose `write objects` step reads Tables announces `dataset.synced`
//! for each (pipelines.md D578, D1050). The row waits here until the server
//! sends it, so a pod lost meanwhile loses no event; sent, it goes.

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, m: &SchemaManager) -> Result<(), DbErr> {
        m.get_connection()
            .execute_unprepared(
                "CREATE TABLE IF NOT EXISTS dataset_syncs ( \
                   id uuid PRIMARY KEY DEFAULT gen_random_uuid(), \
                   org_id bigint NOT NULL REFERENCES organizations(id) ON DELETE CASCADE, \
                   dataset_id uuid NOT NULL REFERENCES datasets(id) ON DELETE CASCADE, \
                   reference text NOT NULL, \
                   counts jsonb NOT NULL, \
                   changed jsonb NOT NULL, \
                   created_at timestamptz NOT NULL DEFAULT now());",
            )
            .await?;
        Ok(())
    }

    async fn down(&self, m: &SchemaManager) -> Result<(), DbErr> {
        m.get_connection()
            .execute_unprepared("DROP TABLE IF EXISTS dataset_syncs;")
            .await?;
        Ok(())
    }
}
