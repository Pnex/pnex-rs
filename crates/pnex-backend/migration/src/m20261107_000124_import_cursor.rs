//! Incremental reads (connectors.md D794): the last cursor value an import committed
//! and how many incremental reads ran since the last full one.

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, m: &SchemaManager) -> Result<(), DbErr> {
        m.get_connection()
            .execute_unprepared(
                "ALTER TABLE object_imports ADD COLUMN cursor text;
                 ALTER TABLE object_imports ADD COLUMN cursor_runs integer NOT NULL DEFAULT 0;",
            )
            .await?;
        Ok(())
    }

    async fn down(&self, m: &SchemaManager) -> Result<(), DbErr> {
        m.get_connection()
            .execute_unprepared(
                "ALTER TABLE object_imports DROP COLUMN cursor_runs;
                 ALTER TABLE object_imports DROP COLUMN cursor;",
            )
            .await?;
        Ok(())
    }
}
