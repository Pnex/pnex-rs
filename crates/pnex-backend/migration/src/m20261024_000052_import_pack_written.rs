//! What a pack last wrote in one of its imports (mapping, source, URL): an
//! upgrade updates an import the org left as is, keeps an edited one
//! (network-check.md D424).

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, m: &SchemaManager) -> Result<(), DbErr> {
        m.get_connection()
            .execute_unprepared("ALTER TABLE object_imports ADD COLUMN pack_written jsonb;")
            .await?;
        Ok(())
    }

    async fn down(&self, m: &SchemaManager) -> Result<(), DbErr> {
        m.get_connection()
            .execute_unprepared("ALTER TABLE object_imports DROP COLUMN IF EXISTS pack_written;")
            .await?;
        Ok(())
    }
}
