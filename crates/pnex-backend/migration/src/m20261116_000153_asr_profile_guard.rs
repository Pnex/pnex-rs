//! ASR profile: a fallback model forced to the profile language redoes the
//! parts the main model answered in another one, and the diarization
//! clustering threshold is set per profile (media.md, language
//! guard and multi-speaker measures of 2026-11-16).

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, m: &SchemaManager) -> Result<(), DbErr> {
        m.get_connection()
            .execute_unprepared(
                "ALTER TABLE asr_profiles
                   ADD COLUMN fallback_asr_model_id uuid
                     REFERENCES ml_models(id) ON DELETE SET NULL,
                   ADD COLUMN cluster_threshold real
                     CHECK (cluster_threshold IS NULL OR cluster_threshold BETWEEN 0.05 AND 1);",
            )
            .await?;
        Ok(())
    }

    async fn down(&self, m: &SchemaManager) -> Result<(), DbErr> {
        m.get_connection()
            .execute_unprepared(
                "ALTER TABLE asr_profiles DROP COLUMN fallback_asr_model_id, DROP COLUMN cluster_threshold;",
            )
            .await?;
        Ok(())
    }
}
