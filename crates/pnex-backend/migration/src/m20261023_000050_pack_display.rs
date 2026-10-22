//! ontology.md D370: the org's labels and colours of the enum values of a
//! pack's properties (`{"<scope>/<property>": {labels, colors}}`), kept
//! across the pack's upgrades; the pack's own ones stay in the pack.

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, m: &SchemaManager) -> Result<(), DbErr> {
        m.get_connection()
            .execute_unprepared(
                "ALTER TABLE packs ADD COLUMN IF NOT EXISTS display jsonb NOT NULL DEFAULT '{}'::jsonb",
            )
            .await?;
        Ok(())
    }

    async fn down(&self, m: &SchemaManager) -> Result<(), DbErr> {
        m.get_connection()
            .execute_unprepared("ALTER TABLE packs DROP COLUMN IF EXISTS display")
            .await?;
        Ok(())
    }
}
