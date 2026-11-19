//! A created object may come with its link (markets-research.md D843): an
//! `object_create` proposal names a link type and the object to link the
//! new one to, both made on acceptance.

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, m: &SchemaManager) -> Result<(), DbErr> {
        m.get_connection()
            .execute_unprepared("ALTER TABLE proposals ADD COLUMN IF NOT EXISTS link_type text;")
            .await?;
        Ok(())
    }

    async fn down(&self, m: &SchemaManager) -> Result<(), DbErr> {
        m.get_connection()
            .execute_unprepared("ALTER TABLE proposals DROP COLUMN IF EXISTS link_type;")
            .await?;
        Ok(())
    }
}
