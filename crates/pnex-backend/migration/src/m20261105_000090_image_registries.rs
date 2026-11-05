//! Image registries (agent-environments.md D703): an OCI registry and repository,
//! with distinct read and write secrets in the vault.

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, m: &SchemaManager) -> Result<(), DbErr> {
        m.get_connection()
            .execute_unprepared(
                "CREATE TABLE image_registries (
                   id uuid PRIMARY KEY,
                   org_id bigint NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
                   name text NOT NULL,
                   host text NOT NULL,
                   repository text NOT NULL,
                   tls boolean NOT NULL DEFAULT true,
                   read_username text,
                   read_secret_id uuid,
                   write_username text,
                   write_secret_id uuid,
                   version integer NOT NULL DEFAULT 1,
                   created_by bigint REFERENCES users(id) ON DELETE SET NULL,
                   created_at timestamptz NOT NULL DEFAULT now(),
                   updated_at timestamptz NOT NULL DEFAULT now(),
                   UNIQUE (org_id, name),
                   CHECK (read_secret_id IS DISTINCT FROM write_secret_id
                          OR read_secret_id IS NULL)
                 );",
            )
            .await?;
        Ok(())
    }

    async fn down(&self, m: &SchemaManager) -> Result<(), DbErr> {
        m.get_connection()
            .execute_unprepared("DROP TABLE image_registries;")
            .await?;
        Ok(())
    }
}
