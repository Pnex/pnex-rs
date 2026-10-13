//! Vision Lab: position of an image in its source (video file, camera
//! recording), which orders the frames for box interpolation (Q3).

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, m: &SchemaManager) -> Result<(), DbErr> {
        m.get_connection()
            .execute_unprepared(
                "ALTER TABLE lab_images ADD COLUMN frame_ms bigint;
                 CREATE INDEX lab_images_source_frame ON lab_images (project_id, source, frame_ms);",
            )
            .await?;
        Ok(())
    }

    async fn down(&self, m: &SchemaManager) -> Result<(), DbErr> {
        m.get_connection()
            .execute_unprepared(
                "DROP INDEX IF EXISTS lab_images_source_frame;
                 ALTER TABLE lab_images DROP COLUMN IF EXISTS frame_ms;",
            )
            .await?;
        Ok(())
    }
}
