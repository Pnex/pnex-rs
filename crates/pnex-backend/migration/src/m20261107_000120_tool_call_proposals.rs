//! A write tool call of an internal agent waits in the proposals inbox (D785).

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, m: &SchemaManager) -> Result<(), DbErr> {
        m.get_connection()
            .execute_unprepared(
                "ALTER TABLE proposals DROP CONSTRAINT proposals_kind;
                 ALTER TABLE proposals ADD CONSTRAINT proposals_kind CHECK (kind IN
                   ('object_create', 'object_update', 'link_create', 'link_update',
                    'link_close', 'case_event', 'object_erase', 'tool_call'));",
            )
            .await?;
        Ok(())
    }

    async fn down(&self, m: &SchemaManager) -> Result<(), DbErr> {
        m.get_connection()
            .execute_unprepared(
                "DELETE FROM proposals WHERE kind = 'tool_call';
                 ALTER TABLE proposals DROP CONSTRAINT proposals_kind;
                 ALTER TABLE proposals ADD CONSTRAINT proposals_kind CHECK (kind IN
                   ('object_create', 'object_update', 'link_create', 'link_update',
                    'link_close', 'case_event', 'object_erase'));",
            )
            .await?;
        Ok(())
    }
}
