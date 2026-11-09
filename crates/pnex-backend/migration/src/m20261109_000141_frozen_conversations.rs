//! Assistant conversations frozen at an instant (markets-research.md D902):
//! every tool answers as of `frozen_at`, never later.

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, m: &SchemaManager) -> Result<(), DbErr> {
        m.get_connection()
            .execute_unprepared("ALTER TABLE ai_conversations ADD COLUMN frozen_at timestamptz;")
            .await?;
        Ok(())
    }

    async fn down(&self, m: &SchemaManager) -> Result<(), DbErr> {
        m.get_connection()
            .execute_unprepared("ALTER TABLE ai_conversations DROP COLUMN frozen_at;")
            .await?;
        Ok(())
    }
}
