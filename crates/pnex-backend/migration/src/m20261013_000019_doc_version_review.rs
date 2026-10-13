//! Named page versions written by flows (media-vision-studio.md D209/D210):
//! whether the version holds AI-generated text, which flow wrote it, and
//! who approved it (an AI report leaves PneX approved by default).

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, m: &SchemaManager) -> Result<(), DbErr> {
        m.get_connection()
            .execute_unprepared(
                "ALTER TABLE doc_versions ADD COLUMN ai_generated boolean NOT NULL DEFAULT false;
                 ALTER TABLE doc_versions ADD COLUMN flow_id bigint;
                 ALTER TABLE doc_versions ADD COLUMN approved_by bigint REFERENCES users(id) ON DELETE SET NULL;
                 ALTER TABLE doc_versions ADD COLUMN approved_at timestamptz;",
            )
            .await?;
        Ok(())
    }

    async fn down(&self, m: &SchemaManager) -> Result<(), DbErr> {
        m.get_connection()
            .execute_unprepared(
                "ALTER TABLE doc_versions DROP COLUMN IF EXISTS approved_at;
                 ALTER TABLE doc_versions DROP COLUMN IF EXISTS approved_by;
                 ALTER TABLE doc_versions DROP COLUMN IF EXISTS flow_id;
                 ALTER TABLE doc_versions DROP COLUMN IF EXISTS ai_generated;",
            )
            .await?;
        Ok(())
    }
}
