//! Org web search providers (web-presence.md D360): school of
//! `browser_endpoints` (key in the vault, one default per org).

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, m: &SchemaManager) -> Result<(), DbErr> {
        m.get_connection()
            .execute_unprepared(
                "CREATE TABLE web_search_providers (
                    id uuid PRIMARY KEY,
                    org_id bigint NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
                    name varchar(255) NOT NULL,
                    kind varchar(32) NOT NULL,
                    base_url varchar(2048) NOT NULL DEFAULT '',
                    secret_id uuid,
                    is_default boolean NOT NULL DEFAULT false,
                    created_by bigint REFERENCES users(id) ON DELETE SET NULL,
                    updated_by bigint REFERENCES users(id) ON DELETE SET NULL,
                    created_at timestamptz NOT NULL DEFAULT now(),
                    updated_at timestamptz NOT NULL DEFAULT now(),
                    UNIQUE (org_id, name)
                );
                CREATE UNIQUE INDEX uniq_web_search_providers_default
                    ON web_search_providers (org_id) WHERE is_default;",
            )
            .await?;
        Ok(())
    }

    async fn down(&self, m: &SchemaManager) -> Result<(), DbErr> {
        m.get_connection()
            .execute_unprepared("DROP TABLE IF EXISTS web_search_providers;")
            .await?;
        Ok(())
    }
}
