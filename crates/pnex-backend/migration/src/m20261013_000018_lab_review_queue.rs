//! Vision Lab review queue (active learning, M4): pictures sent by a
//! `vision-detect` node when the model hesitated, with its proposals,
//! until someone annotates them.

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, m: &SchemaManager) -> Result<(), DbErr> {
        m.get_connection()
            .execute_unprepared(
                "ALTER TABLE lab_images ADD COLUMN review boolean NOT NULL DEFAULT false;
                 ALTER TABLE lab_images ADD COLUMN proposals jsonb NOT NULL DEFAULT '[]'::jsonb;
                 CREATE INDEX lab_images_review ON lab_images (project_id) WHERE review;",
            )
            .await?;
        Ok(())
    }

    async fn down(&self, m: &SchemaManager) -> Result<(), DbErr> {
        m.get_connection()
            .execute_unprepared(
                "DROP INDEX IF EXISTS lab_images_review;
                 ALTER TABLE lab_images DROP COLUMN IF EXISTS proposals;
                 ALTER TABLE lab_images DROP COLUMN IF EXISTS review;",
            )
            .await?;
        Ok(())
    }
}
