//! Vision Lab diversity guidance (M4): mean luma of each picture, measured
//! at import, to warn when every picture has the same lighting.

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, m: &SchemaManager) -> Result<(), DbErr> {
        m.get_connection()
            .execute_unprepared("ALTER TABLE lab_images ADD COLUMN luma real;")
            .await?;
        Ok(())
    }

    async fn down(&self, m: &SchemaManager) -> Result<(), DbErr> {
        m.get_connection()
            .execute_unprepared("ALTER TABLE lab_images DROP COLUMN IF EXISTS luma;")
            .await?;
        Ok(())
    }
}
