//! Incoming webhooks (connectors.md D795): the kind, and the inbox that keeps each
//! signed call until an import reads it into a table.

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
                   CHECK (kind IN ('postgres', 'rest', 'webhook'));
                 CREATE TABLE connector_inbox (
                   id bigserial PRIMARY KEY,
                   org_id bigint NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
                   connector_id uuid NOT NULL REFERENCES connectors(id) ON DELETE CASCADE,
                   body jsonb NOT NULL,
                   received_at timestamptz NOT NULL DEFAULT now()
                 );
                 CREATE INDEX connector_inbox_by_connector ON connector_inbox (connector_id, id);",
            )
            .await?;
        Ok(())
    }

    async fn down(&self, m: &SchemaManager) -> Result<(), DbErr> {
        m.get_connection()
            .execute_unprepared(
                "DROP TABLE connector_inbox;
                 DELETE FROM connectors WHERE kind = 'webhook';
                 ALTER TABLE connectors DROP CONSTRAINT connectors_kind_check;
                 ALTER TABLE connectors ADD CONSTRAINT connectors_kind_check
                   CHECK (kind IN ('postgres', 'rest'));",
            )
            .await?;
        Ok(())
    }
}
