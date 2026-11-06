//! How long replaced versions of a pipeline output stay readable, set by the
//! transform that writes it (data-pipelines.md). Null: the default, 90 days.

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, m: &SchemaManager) -> Result<(), DbErr> {
        m.get_connection()
            .execute_unprepared(
                "ALTER TABLE datasets ADD COLUMN retain_days integer
                   CHECK (retain_days BETWEEN 1 AND 3650);",
            )
            .await?;
        Ok(())
    }

    async fn down(&self, m: &SchemaManager) -> Result<(), DbErr> {
        m.get_connection()
            .execute_unprepared("ALTER TABLE datasets DROP COLUMN retain_days;")
            .await?;
        Ok(())
    }
}
