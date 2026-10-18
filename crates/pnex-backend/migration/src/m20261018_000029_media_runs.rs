//! Runs of a media file's transcription (media-review.md D276): the hidden
//! batch stream belongs to the asset (a post of a platform has its
//! subtitles before any video file), each run says where its text comes
//! from (`pnex` ASR on a version, or the `platform` subtitles).

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, m: &SchemaManager) -> Result<(), DbErr> {
        m.get_connection()
            .execute_unprepared(
                "ALTER TABLE media_streams
                    ADD COLUMN media_asset_id uuid REFERENCES media_assets(id) ON DELETE CASCADE;
                UPDATE media_streams s SET media_asset_id = v.asset_id
                    FROM media_versions v WHERE v.id = s.media_version_id;
                DROP INDEX IF EXISTS uniq_media_streams_batch_version;
                ALTER TABLE media_streams DROP CONSTRAINT IF EXISTS media_streams_media_version_id_fkey;
                ALTER TABLE media_streams ADD CONSTRAINT media_streams_media_version_id_fkey
                    FOREIGN KEY (media_version_id) REFERENCES media_versions(id) ON DELETE SET NULL;
                CREATE UNIQUE INDEX uniq_media_streams_batch_asset
                    ON media_streams (media_asset_id) WHERE media_asset_id IS NOT NULL;
                CREATE TABLE media_runs (
                    id uuid PRIMARY KEY,
                    org_id bigint NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
                    media_asset_id uuid NOT NULL REFERENCES media_assets(id) ON DELETE CASCADE,
                    media_version_id uuid REFERENCES media_versions(id) ON DELETE SET NULL,
                    origin varchar(16) NOT NULL,
                    lang varchar(16),
                    created_by bigint,
                    created_at timestamptz NOT NULL DEFAULT now()
                );
                CREATE INDEX idx_media_runs_asset ON media_runs (org_id, media_asset_id, created_at);",
            )
            .await?;
        Ok(())
    }

    async fn down(&self, m: &SchemaManager) -> Result<(), DbErr> {
        m.get_connection()
            .execute_unprepared(
                "DROP TABLE IF EXISTS media_runs;
                DROP INDEX IF EXISTS uniq_media_streams_batch_asset;
                ALTER TABLE media_streams DROP COLUMN IF EXISTS media_asset_id;",
            )
            .await?;
        Ok(())
    }
}
