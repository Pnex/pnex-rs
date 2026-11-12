//! The state of each enumerated row of a pipeline between runs
//! (pipeline-engine.md D937): one row per (org, pipeline, item). The item
//! `''` holds the outcome of the last run, `@sha:…` the bytes last read by a
//! fetch that skips an unchanged source.

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, m: &SchemaManager) -> Result<(), DbErr> {
        m.get_connection()
            .execute_unprepared(
                "CREATE TABLE pipeline_items (
                   org_id bigint NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
                   pipeline text NOT NULL,
                   item text NOT NULL,
                   state text NOT NULL CHECK (state IN ('done', 'empty', 'failed')),
                   code text,
                   args jsonb,
                   retry_after timestamptz,
                   failures integer NOT NULL DEFAULT 0,
                   rows bigint NOT NULL DEFAULT 0,
                   sha256 text,
                   updated_at timestamptz NOT NULL DEFAULT now(),
                   PRIMARY KEY (org_id, pipeline, item)
                 );",
            )
            .await?;
        Ok(())
    }

    async fn down(&self, m: &SchemaManager) -> Result<(), DbErr> {
        m.get_connection()
            .execute_unprepared("DROP TABLE pipeline_items;")
            .await?;
        Ok(())
    }
}
