//! Per-object imports take the objects in turn (account-mapping.md D331):
//! the last value queried by a scheduled run, the next run starts after it.

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, m: &SchemaManager) -> Result<(), DbErr> {
        m.get_connection()
            .execute_unprepared("ALTER TABLE object_imports ADD COLUMN per_object_cursor text;")
            .await?;
        Ok(())
    }

    async fn down(&self, m: &SchemaManager) -> Result<(), DbErr> {
        m.get_connection()
            .execute_unprepared(
                "ALTER TABLE object_imports DROP COLUMN IF EXISTS per_object_cursor;",
            )
            .await?;
        Ok(())
    }
}
