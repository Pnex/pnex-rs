//! REST connectors (connectors.md D793): a kind beyond `postgres` and the settings it
//! needs besides host, port and TLS.

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
                   CHECK (kind IN ('postgres', 'rest'));
                 ALTER TABLE connectors ADD COLUMN settings jsonb NOT NULL DEFAULT '{}';",
            )
            .await?;
        Ok(())
    }

    async fn down(&self, m: &SchemaManager) -> Result<(), DbErr> {
        m.get_connection()
            .execute_unprepared(
                "DELETE FROM connectors WHERE kind <> 'postgres';
                 ALTER TABLE connectors DROP COLUMN settings;
                 ALTER TABLE connectors DROP CONSTRAINT connectors_kind_check;
                 ALTER TABLE connectors ADD CONSTRAINT connectors_kind_check
                   CHECK (kind IN ('postgres'));",
            )
            .await?;
        Ok(())
    }
}
