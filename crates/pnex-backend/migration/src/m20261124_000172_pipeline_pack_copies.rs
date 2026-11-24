//! A pack copies each of its imports into an org pipeline (pipelines.md
//! D1050): `pack` names the pack that wrote it, `pack_written` what it wrote
//! last, so an upgrade replaces only a copy the org left as is (D424).

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, m: &SchemaManager) -> Result<(), DbErr> {
        m.get_connection()
            .execute_unprepared(
                "ALTER TABLE pipelines ADD COLUMN pack text, ADD COLUMN pack_written jsonb;",
            )
            .await?;
        Ok(())
    }

    async fn down(&self, m: &SchemaManager) -> Result<(), DbErr> {
        m.get_connection()
            .execute_unprepared(
                "ALTER TABLE pipelines DROP COLUMN IF EXISTS pack, DROP COLUMN IF EXISTS pack_written;",
            )
            .await?;
        Ok(())
    }
}
