//! Who wrote each version of a Table (pipelines.md D1050, SEC-93): a
//! `write objects` step writes with the lowest role of the members who
//! last loaded the Tables it reads. An upload keeps its sender until its
//! conversion. NULL: a pack, the system, or a member gone.

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, m: &SchemaManager) -> Result<(), DbErr> {
        m.get_connection()
            .execute_unprepared(
                "ALTER TABLE dataset_transactions \
                   ADD COLUMN IF NOT EXISTS created_by bigint REFERENCES users(id) ON DELETE SET NULL; \
                 ALTER TABLE dataset_uploads \
                   ADD COLUMN IF NOT EXISTS created_by bigint REFERENCES users(id) ON DELETE SET NULL;",
            )
            .await?;
        Ok(())
    }

    async fn down(&self, m: &SchemaManager) -> Result<(), DbErr> {
        m.get_connection()
            .execute_unprepared(
                "ALTER TABLE dataset_transactions DROP COLUMN IF EXISTS created_by; \
                 ALTER TABLE dataset_uploads DROP COLUMN IF EXISTS created_by;",
            )
            .await?;
        Ok(())
    }
}
