//! Connectors (data-pipelines.md D352): saved access to an outside database, its
//! password in the vault, and the private hosts the platform admin lets an org reach.

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, m: &SchemaManager) -> Result<(), DbErr> {
        m.get_connection()
            .execute_unprepared(
                "CREATE TABLE connectors (
                   id uuid PRIMARY KEY,
                   org_id bigint NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
                   name text NOT NULL,
                   kind text NOT NULL CHECK (kind IN ('postgres')),
                   host text NOT NULL,
                   port integer NOT NULL,
                   database text NOT NULL,
                   username text NOT NULL,
                   tls boolean NOT NULL DEFAULT true,
                   secret_id uuid,
                   version integer NOT NULL DEFAULT 1,
                   created_by bigint REFERENCES users(id) ON DELETE SET NULL,
                   created_at timestamptz NOT NULL DEFAULT now(),
                   updated_at timestamptz NOT NULL DEFAULT now(),
                   UNIQUE (org_id, name)
                 );
                 CREATE TABLE connector_allowed_hosts (
                   org_id bigint NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
                   host text NOT NULL,
                   updated_by bigint REFERENCES users(id) ON DELETE SET NULL,
                   created_at timestamptz NOT NULL DEFAULT now(),
                   PRIMARY KEY (org_id, host)
                 );",
            )
            .await?;
        Ok(())
    }

    async fn down(&self, m: &SchemaManager) -> Result<(), DbErr> {
        m.get_connection()
            .execute_unprepared("DROP TABLE connector_allowed_hosts; DROP TABLE connectors;")
            .await?;
        Ok(())
    }
}
