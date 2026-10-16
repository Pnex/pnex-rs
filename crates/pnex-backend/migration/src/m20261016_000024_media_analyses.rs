//! File analyses (media-import-pipeline.md D213): one row per (media
//! version, step). The row is the visible state of a step; the work is
//! delegated to the existing machinery (index job, LLM provider, ...).

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, m: &SchemaManager) -> Result<(), DbErr> {
        m.get_connection()
            .execute_unprepared(
                "CREATE TABLE media_analyses (
                    id uuid PRIMARY KEY,
                    org_id bigint NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
                    media_version_id uuid NOT NULL REFERENCES media_versions(id) ON DELETE CASCADE,
                    step varchar(32) NOT NULL,
                    state varchar(16) NOT NULL,
                    progress smallint NOT NULL DEFAULT 0,
                    error_code varchar(64),
                    params jsonb NOT NULL DEFAULT '{}'::jsonb,
                    result_ref jsonb NOT NULL DEFAULT '{}'::jsonb,
                    created_by bigint,
                    created_at timestamptz NOT NULL DEFAULT now(),
                    updated_at timestamptz NOT NULL DEFAULT now(),
                    UNIQUE (media_version_id, step)
                );
                CREATE INDEX idx_media_analyses_org_state ON media_analyses (org_id, state);",
            )
            .await?;
        Ok(())
    }

    async fn down(&self, m: &SchemaManager) -> Result<(), DbErr> {
        m.get_connection()
            .execute_unprepared("DROP TABLE IF EXISTS media_analyses;")
            .await?;
        Ok(())
    }
}
