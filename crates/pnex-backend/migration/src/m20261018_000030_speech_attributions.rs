//! Spans of a media attributed to persons of the ontology (media-review.md
//! D272): small rows edited by people, the text stays in O2.

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, m: &SchemaManager) -> Result<(), DbErr> {
        m.get_connection()
            .execute_unprepared(
                "CREATE TABLE speech_attributions (
                    id uuid PRIMARY KEY,
                    org_id bigint NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
                    media_asset_id uuid NOT NULL REFERENCES media_assets(id) ON DELETE CASCADE,
                    object_id uuid NOT NULL REFERENCES objects(id) ON DELETE CASCADE,
                    start_ms bigint NOT NULL,
                    end_ms bigint NOT NULL,
                    origin varchar(16) NOT NULL,
                    confidence real,
                    status varchar(16) NOT NULL,
                    version integer NOT NULL DEFAULT 1,
                    created_by bigint,
                    created_at timestamptz NOT NULL DEFAULT now(),
                    updated_at timestamptz NOT NULL DEFAULT now(),
                    CHECK (start_ms >= 0 AND end_ms > start_ms)
                );
                CREATE INDEX idx_speech_attributions_asset
                    ON speech_attributions (org_id, media_asset_id, start_ms);
                CREATE INDEX idx_speech_attributions_object ON speech_attributions (object_id);",
            )
            .await?;
        Ok(())
    }

    async fn down(&self, m: &SchemaManager) -> Result<(), DbErr> {
        m.get_connection()
            .execute_unprepared("DROP TABLE IF EXISTS speech_attributions;")
            .await?;
        Ok(())
    }
}
