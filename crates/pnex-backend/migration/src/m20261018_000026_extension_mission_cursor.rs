//! Progress of a browser agent mission (extension-collector.md D256), written
//! by the server only: the document of a `collect_page` mission and the
//! digest of its last text (an unchanged page adds no version).

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, m: &SchemaManager) -> Result<(), DbErr> {
        m.get_connection()
            .execute_unprepared("ALTER TABLE extension_missions ADD COLUMN cursor jsonb;")
            .await?;
        Ok(())
    }

    async fn down(&self, m: &SchemaManager) -> Result<(), DbErr> {
        m.get_connection()
            .execute_unprepared("ALTER TABLE extension_missions DROP COLUMN IF EXISTS cursor;")
            .await?;
        Ok(())
    }
}
