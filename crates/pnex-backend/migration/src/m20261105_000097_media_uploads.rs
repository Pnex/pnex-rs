//! A media file sent in several requests (data-pipelines.md §9.9): a proxy limits each
//! request, Cloudflare to 100 MB. Parts wait on S3, then a server joins them.

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, m: &SchemaManager) -> Result<(), DbErr> {
        m.get_connection()
            .execute_unprepared(
                "CREATE TABLE media_uploads (
                   id uuid PRIMARY KEY DEFAULT gen_random_uuid(),
                   org_id bigint NOT NULL,
                   -- Empty for a new asset until it is done.
                   asset_id uuid REFERENCES media_assets(id) ON DELETE CASCADE,
                   filename text NOT NULL,
                   name text,
                   kind text,
                   content_type text,
                   note text,
                   state text NOT NULL DEFAULT 'open'
                     CHECK (state IN ('open', 'ready', 'joining', 'done', 'failed')),
                   parts jsonb NOT NULL DEFAULT '{}'::jsonb,
                   version_number bigint,
                   code text,
                   args jsonb,
                   attempts integer NOT NULL DEFAULT 0,
                   heartbeat_at timestamptz,
                   created_at timestamptz NOT NULL DEFAULT now(),
                   updated_at timestamptz NOT NULL DEFAULT now()
                 );
                 CREATE INDEX media_uploads_org ON media_uploads (org_id, state);
                 CREATE INDEX media_uploads_state ON media_uploads (state, updated_at);",
            )
            .await?;
        Ok(())
    }

    async fn down(&self, m: &SchemaManager) -> Result<(), DbErr> {
        m.get_connection()
            .execute_unprepared("DROP TABLE media_uploads;")
            .await?;
        Ok(())
    }
}
