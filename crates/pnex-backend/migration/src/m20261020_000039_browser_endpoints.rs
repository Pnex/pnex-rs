//! Org browser endpoints (site-recipes.md D325): CDP browsers the org plays
//! its navigation rules on. The token lives in the vault (`secret_id`,
//! school of `geo_providers`); one default per org.

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, m: &SchemaManager) -> Result<(), DbErr> {
        m.get_connection()
            .execute_unprepared(
                "CREATE TABLE browser_endpoints (
                    id uuid PRIMARY KEY,
                    org_id bigint NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
                    name varchar(255) NOT NULL,
                    cdp_url varchar(2048) NOT NULL,
                    secret_id uuid,
                    max_connections integer NOT NULL DEFAULT 2,
                    is_default boolean NOT NULL DEFAULT false,
                    created_by bigint REFERENCES users(id) ON DELETE SET NULL,
                    updated_by bigint REFERENCES users(id) ON DELETE SET NULL,
                    created_at timestamptz NOT NULL DEFAULT now(),
                    updated_at timestamptz NOT NULL DEFAULT now(),
                    UNIQUE (org_id, name)
                );
                CREATE UNIQUE INDEX uniq_browser_endpoints_default
                    ON browser_endpoints (org_id) WHERE is_default;",
            )
            .await?;
        Ok(())
    }

    async fn down(&self, m: &SchemaManager) -> Result<(), DbErr> {
        m.get_connection()
            .execute_unprepared("DROP TABLE IF EXISTS browser_endpoints;")
            .await?;
        Ok(())
    }
}
