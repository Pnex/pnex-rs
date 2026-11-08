//! Detection of personal data in tables (governance.md D804): proposals to mark a
//! table, and the last version of each table the detection read.

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
                    'link_close', 'case_event', 'object_erase', 'tool_call', 'dataset_mark'));
                 ALTER TABLE datasets ADD COLUMN detected_seq bigint NOT NULL DEFAULT 0;",
            )
            .await?;
        Ok(())
    }

    async fn down(&self, m: &SchemaManager) -> Result<(), DbErr> {
        m.get_connection()
            .execute_unprepared(
                "ALTER TABLE datasets DROP COLUMN detected_seq;
                 DELETE FROM proposals WHERE kind = 'dataset_mark';
                 ALTER TABLE proposals DROP CONSTRAINT proposals_kind;
                 ALTER TABLE proposals ADD CONSTRAINT proposals_kind CHECK (kind IN
                   ('object_create', 'object_update', 'link_create', 'link_update',
                    'link_close', 'case_event', 'object_erase', 'tool_call'));",
            )
            .await?;
        Ok(())
    }
}
