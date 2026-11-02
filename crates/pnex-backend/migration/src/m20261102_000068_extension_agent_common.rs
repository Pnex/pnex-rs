//! Personal or common browser agent (extension-collector.md D550): an agent
//! runs the missions of the member who paired it (`paired_by`) unless an
//! admin made it common.

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, m: &SchemaManager) -> Result<(), DbErr> {
        m.get_connection()
            .execute_unprepared(
                "ALTER TABLE extension_agents ADD COLUMN common boolean NOT NULL DEFAULT false;
                 CREATE INDEX idx_extension_agents_owner ON extension_agents (org_id, paired_by);",
            )
            .await?;
        Ok(())
    }

    async fn down(&self, m: &SchemaManager) -> Result<(), DbErr> {
        m.get_connection()
            .execute_unprepared(
                "DROP INDEX IF EXISTS idx_extension_agents_owner;
                 ALTER TABLE extension_agents DROP COLUMN IF EXISTS common;",
            )
            .await?;
        Ok(())
    }
}
