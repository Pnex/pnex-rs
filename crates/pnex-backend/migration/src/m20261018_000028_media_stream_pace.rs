//! Batch analysis of a library file (media-vision-studio.md D211,
//! media-review.md D276): a stream reads its file as fast as the ASR keeps
//! up (`pace = batch`), on the analysed version, and each launch is a run.

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, m: &SchemaManager) -> Result<(), DbErr> {
        m.get_connection()
            .execute_unprepared(
                "ALTER TABLE media_streams
                    ADD COLUMN pace varchar(16) NOT NULL DEFAULT 'realtime',
                    ADD COLUMN media_version_id uuid REFERENCES media_versions(id) ON DELETE CASCADE,
                    ADD COLUMN run_id uuid;
                CREATE UNIQUE INDEX uniq_media_streams_batch_version
                    ON media_streams (media_version_id) WHERE media_version_id IS NOT NULL;",
            )
            .await?;
        Ok(())
    }

    async fn down(&self, m: &SchemaManager) -> Result<(), DbErr> {
        m.get_connection()
            .execute_unprepared(
                "ALTER TABLE media_streams
                    DROP COLUMN IF EXISTS run_id,
                    DROP COLUMN IF EXISTS media_version_id,
                    DROP COLUMN IF EXISTS pace;",
            )
            .await?;
        Ok(())
    }
}
