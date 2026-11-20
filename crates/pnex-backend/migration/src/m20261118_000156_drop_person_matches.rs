//! Person matches run as a pack pipeline (pipelines.md D996): their
//! state is the pipeline's rows, navigations and proposals, not a table.

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, m: &SchemaManager) -> Result<(), DbErr> {
        m.get_connection()
            .execute_unprepared("DROP TABLE IF EXISTS person_matches;")
            .await?;
        Ok(())
    }

    async fn down(&self, m: &SchemaManager) -> Result<(), DbErr> {
        crate::m20261107_000122_person_matches::Migration
            .up(m)
            .await
    }
}
