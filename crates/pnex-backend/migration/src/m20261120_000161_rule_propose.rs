//! A rule may propose an object when it fires (analysis.md D829): `propose` is
//! `{"type_key", "title", "link_type"}`, absent for a rule that only notifies.

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, m: &SchemaManager) -> Result<(), DbErr> {
        m.get_connection()
            .execute_unprepared("ALTER TABLE rules ADD COLUMN IF NOT EXISTS propose jsonb;")
            .await?;
        Ok(())
    }

    async fn down(&self, m: &SchemaManager) -> Result<(), DbErr> {
        m.get_connection()
            .execute_unprepared("ALTER TABLE rules DROP COLUMN IF EXISTS propose;")
            .await?;
        Ok(())
    }
}
