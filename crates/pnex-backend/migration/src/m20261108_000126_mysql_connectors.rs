//! MySQL connectors, under TLS only (connectors.md D792).

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, m: &SchemaManager) -> Result<(), DbErr> {
        m.get_connection()
            .execute_unprepared(
                "ALTER TABLE connectors DROP CONSTRAINT connectors_kind_check;
                 ALTER TABLE connectors ADD CONSTRAINT connectors_kind_check
                   CHECK (kind IN ('postgres', 'rest', 'webhook', 'mysql'));",
            )
            .await?;
        Ok(())
    }

    async fn down(&self, m: &SchemaManager) -> Result<(), DbErr> {
        m.get_connection()
            .execute_unprepared(
                "DELETE FROM connectors WHERE kind = 'mysql';
                 ALTER TABLE connectors DROP CONSTRAINT connectors_kind_check;
                 ALTER TABLE connectors ADD CONSTRAINT connectors_kind_check
                   CHECK (kind IN ('postgres', 'rest', 'webhook'));",
            )
            .await?;
        Ok(())
    }
}
