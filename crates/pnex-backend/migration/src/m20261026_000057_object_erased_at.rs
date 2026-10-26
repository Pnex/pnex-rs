//! Erased objects (crm.md D446): the shell keeps its links and provenance,
//! its personal data is gone and its history is read redacted. Erasures are
//! proposed by the retention queue (D445): a new proposal kind.

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, m: &SchemaManager) -> Result<(), DbErr> {
        m.get_connection()
            .execute_unprepared(
                "ALTER TABLE objects ADD COLUMN erased_at timestamptz;
                 ALTER TABLE proposals DROP CONSTRAINT proposals_kind;
                 ALTER TABLE proposals ADD CONSTRAINT proposals_kind CHECK (kind IN
                   ('object_create', 'object_update', 'link_create', 'link_update',
                    'link_close', 'case_event', 'object_erase'));",
            )
            .await?;
        Ok(())
    }

    async fn down(&self, m: &SchemaManager) -> Result<(), DbErr> {
        m.get_connection()
            .execute_unprepared(
                "DELETE FROM proposals WHERE kind = 'object_erase';
                 ALTER TABLE proposals DROP CONSTRAINT proposals_kind;
                 ALTER TABLE proposals ADD CONSTRAINT proposals_kind CHECK (kind IN
                   ('object_create', 'object_update', 'link_create', 'link_update',
                    'link_close', 'case_event'));
                 ALTER TABLE objects DROP COLUMN IF EXISTS erased_at;",
            )
            .await?;
        Ok(())
    }
}
