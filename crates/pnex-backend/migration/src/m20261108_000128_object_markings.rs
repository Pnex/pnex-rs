//! Markings of ontology objects (governance.md D800–D802): set by hand, or by the
//! sync of a marked table.

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, m: &SchemaManager) -> Result<(), DbErr> {
        m.get_connection()
            .execute_unprepared(
                "CREATE TABLE object_markings (
                   object_id uuid NOT NULL REFERENCES objects(id) ON DELETE CASCADE,
                   marking_id uuid NOT NULL REFERENCES markings(id) ON DELETE RESTRICT,
                   PRIMARY KEY (object_id, marking_id)
                 );
                 CREATE INDEX object_markings_marking ON object_markings (marking_id);",
            )
            .await?;
        Ok(())
    }

    async fn down(&self, m: &SchemaManager) -> Result<(), DbErr> {
        m.get_connection()
            .execute_unprepared("DROP TABLE object_markings;")
            .await?;
        Ok(())
    }
}
