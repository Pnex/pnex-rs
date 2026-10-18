//! Video registry of the browser agents (extension-collector.md D259): one
//! row per post an agent saw, unique per (org, platform, post), so no agent
//! downloads the same video twice.

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, m: &SchemaManager) -> Result<(), DbErr> {
        m.get_connection()
            .execute_unprepared(
                "CREATE TABLE social_videos (
                    id uuid PRIMARY KEY,
                    org_id bigint NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
                    platform varchar(16) NOT NULL,
                    platform_id varchar(64) NOT NULL,
                    url text NOT NULL,
                    author varchar(64) NOT NULL,
                    text text NOT NULL DEFAULT '',
                    published_at timestamptz,
                    video_url text,
                    duration_ms bigint,
                    state varchar(16) NOT NULL DEFAULT 'seen',
                    media_asset_id uuid REFERENCES media_assets(id) ON DELETE SET NULL,
                    mission_id uuid REFERENCES extension_missions(id) ON DELETE SET NULL,
                    first_seen_at timestamptz NOT NULL DEFAULT now(),
                    captured_at timestamptz,
                    CONSTRAINT uniq_social_videos_post UNIQUE (org_id, platform, platform_id)
                );
                CREATE INDEX idx_social_videos_org_seen ON social_videos (org_id, first_seen_at DESC);",
            )
            .await?;
        Ok(())
    }

    async fn down(&self, m: &SchemaManager) -> Result<(), DbErr> {
        m.get_connection()
            .execute_unprepared("DROP TABLE IF EXISTS social_videos;")
            .await?;
        Ok(())
    }
}
