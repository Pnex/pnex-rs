//! Sync from a dataset (data-pipelines.md D577): the transactions a run read,
//! which the next run compares against.

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, m: &SchemaManager) -> Result<(), DbErr> {
        m.get_connection()
            .execute_unprepared(
                "ALTER TABLE object_import_runs ADD COLUMN dataset_transactions uuid[];",
            )
            .await?;
        Ok(())
    }

    async fn down(&self, m: &SchemaManager) -> Result<(), DbErr> {
        m.get_connection()
            .execute_unprepared(
                "DELETE FROM object_imports WHERE source = 'dataset';
                 ALTER TABLE object_import_runs DROP COLUMN dataset_transactions;",
            )
            .await?;
        Ok(())
    }
}
