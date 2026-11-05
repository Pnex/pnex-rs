//! Who saved each version of a pipeline (data-pipelines.md D354): the studio shows it.

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, m: &SchemaManager) -> Result<(), DbErr> {
        m.get_connection()
            .execute_unprepared(
                "ALTER TABLE pipeline_versions
                   ADD COLUMN author_id bigint REFERENCES users(id) ON DELETE SET NULL;",
            )
            .await?;
        Ok(())
    }

    async fn down(&self, m: &SchemaManager) -> Result<(), DbErr> {
        m.get_connection()
            .execute_unprepared("ALTER TABLE pipeline_versions DROP COLUMN author_id;")
            .await?;
        Ok(())
    }
}
