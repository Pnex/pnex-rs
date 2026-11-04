//! Sync from a dataset (data-pipelines.md D577): what a run planned and could not
//! write because an object it needs did not exist. The source does not change when
//! that object appears, so the next run must not count these as already applied.

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, m: &SchemaManager) -> Result<(), DbErr> {
        m.get_connection()
            .execute_unprepared("ALTER TABLE object_import_runs ADD COLUMN skipped_keys text[];")
            .await?;
        Ok(())
    }

    async fn down(&self, m: &SchemaManager) -> Result<(), DbErr> {
        m.get_connection()
            .execute_unprepared("ALTER TABLE object_import_runs DROP COLUMN skipped_keys;")
            .await?;
        Ok(())
    }
}
