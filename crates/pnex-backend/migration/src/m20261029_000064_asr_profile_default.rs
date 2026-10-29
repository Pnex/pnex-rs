//! The org's default transcription profile: what a capture without its own
//! choice uses (the tray recorder, call-recording.md R1). At most one per
//! org, enforced by a partial unique index.

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, m: &SchemaManager) -> Result<(), DbErr> {
        m.get_connection()
            .execute_unprepared(
                r"ALTER TABLE asr_profiles ADD COLUMN is_default boolean NOT NULL DEFAULT false;
                  CREATE UNIQUE INDEX asr_profiles_one_default ON asr_profiles (org_id) WHERE is_default;",
            )
            .await?;
        Ok(())
    }

    async fn down(&self, m: &SchemaManager) -> Result<(), DbErr> {
        m.get_connection()
            .execute_unprepared(
                r"DROP INDEX IF EXISTS asr_profiles_one_default;
                  ALTER TABLE asr_profiles DROP COLUMN IF EXISTS is_default;",
            )
            .await?;
        Ok(())
    }
}
