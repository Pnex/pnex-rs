//! ontology.md D368: the org's layer over a pack, its overridden settings
//! (`{key: value}`); the pack's own defaults stay in the pack.

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, m: &SchemaManager) -> Result<(), DbErr> {
        m.get_connection()
            .execute_unprepared(
                "ALTER TABLE packs ADD COLUMN IF NOT EXISTS settings jsonb NOT NULL DEFAULT '{}'::jsonb",
            )
            .await?;
        Ok(())
    }

    async fn down(&self, m: &SchemaManager) -> Result<(), DbErr> {
        m.get_connection()
            .execute_unprepared("ALTER TABLE packs DROP COLUMN IF EXISTS settings")
            .await?;
        Ok(())
    }
}
