//! A file sent in several requests (data-pipelines.md D569): a proxy limits each
//! request, Cloudflare to 100 MB. Parts wait on S3, then the server converts them.

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, m: &SchemaManager) -> Result<(), DbErr> {
        m.get_connection()
            .execute_unprepared(
                "CREATE TABLE dataset_uploads (
                   id uuid PRIMARY KEY DEFAULT gen_random_uuid(),
                   org_id bigint NOT NULL,
                   dataset_id uuid NOT NULL REFERENCES datasets(id) ON DELETE CASCADE,
                   kind text NOT NULL CHECK (kind IN ('snapshot', 'append')),
                   delimiter text,
                   state text NOT NULL DEFAULT 'open'
                     CHECK (state IN ('open', 'ready', 'converting', 'done', 'failed')),
                   parts jsonb NOT NULL DEFAULT '{}'::jsonb,
                   transaction_id uuid,
                   code text,
                   args jsonb,
                   attempts integer NOT NULL DEFAULT 0,
                   heartbeat_at timestamptz,
                   created_at timestamptz NOT NULL DEFAULT now(),
                   updated_at timestamptz NOT NULL DEFAULT now()
                 );
                 CREATE UNIQUE INDEX dataset_uploads_one_active
                   ON dataset_uploads (dataset_id)
                   WHERE state IN ('open', 'ready', 'converting');
                 CREATE INDEX dataset_uploads_state ON dataset_uploads (state, updated_at);",
            )
            .await?;
        Ok(())
    }

    async fn down(&self, m: &SchemaManager) -> Result<(), DbErr> {
        m.get_connection()
            .execute_unprepared("DROP TABLE dataset_uploads;")
            .await?;
        Ok(())
    }
}
