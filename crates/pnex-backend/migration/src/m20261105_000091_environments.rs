//! Agent environments (agent-environments.md D702, D704): a Dockerfile and its text
//! context, versioned in the database. Immutable versions, numbered per environment.

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, m: &SchemaManager) -> Result<(), DbErr> {
        m.get_connection()
            .execute_unprepared(
                "CREATE TABLE environments (
                   id uuid PRIMARY KEY,
                   org_id bigint NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
                   name text NOT NULL,
                   description text NOT NULL DEFAULT '',
                   registry_id uuid NOT NULL REFERENCES image_registries(id) ON DELETE RESTRICT,
                   latest_version integer,
                   version integer NOT NULL DEFAULT 1,
                   created_by bigint REFERENCES users(id) ON DELETE SET NULL,
                   created_at timestamptz NOT NULL DEFAULT now(),
                   updated_at timestamptz NOT NULL DEFAULT now(),
                   UNIQUE (org_id, name)
                 );
                 CREATE TABLE environment_versions (
                   environment_id uuid NOT NULL REFERENCES environments(id) ON DELETE CASCADE,
                   number integer NOT NULL,
                   dockerfile text NOT NULL,
                   files jsonb NOT NULL DEFAULT '{}',
                   build_args jsonb NOT NULL DEFAULT '{}',
                   fingerprint text NOT NULL,
                   created_by bigint REFERENCES users(id) ON DELETE SET NULL,
                   created_at timestamptz NOT NULL DEFAULT now(),
                   PRIMARY KEY (environment_id, number)
                 );",
            )
            .await?;
        Ok(())
    }

    async fn down(&self, m: &SchemaManager) -> Result<(), DbErr> {
        m.get_connection()
            .execute_unprepared("DROP TABLE environment_versions; DROP TABLE environments;")
            .await?;
        Ok(())
    }
}
