//! Site navigation rules and their run journal (site-recipes.md D323,
//! D326): a rule belongs to one web site (normalised host), is plain YAML
//! with a version bumped on each save; a run keeps the YAML it executed
//! (a studio test run plays an unsaved draft), its steps, extracts and the
//! storage keys of its screenshots and PDFs.

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, m: &SchemaManager) -> Result<(), DbErr> {
        m.get_connection()
            .execute_unprepared(
                "CREATE TABLE site_recipes (
                    id uuid PRIMARY KEY,
                    org_id bigint NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
                    site varchar(255) NOT NULL,
                    name varchar(120) NOT NULL,
                    yaml text NOT NULL,
                    version integer NOT NULL DEFAULT 1,
                    created_by bigint,
                    updated_by bigint,
                    created_at timestamptz NOT NULL DEFAULT now(),
                    updated_at timestamptz NOT NULL DEFAULT now(),
                    UNIQUE (org_id, site, name)
                );
                CREATE INDEX idx_site_recipes_site ON site_recipes (org_id, site);
                CREATE TABLE site_recipe_runs (
                    id uuid PRIMARY KEY,
                    org_id bigint NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
                    recipe_id uuid REFERENCES site_recipes(id) ON DELETE SET NULL,
                    recipe_version integer,
                    site varchar(255) NOT NULL,
                    name varchar(120) NOT NULL,
                    yaml text NOT NULL,
                    params jsonb NOT NULL DEFAULT '{}',
                    trigger varchar(16) NOT NULL,
                    status varchar(16) NOT NULL,
                    failed_step integer,
                    reason varchar(48),
                    steps jsonb NOT NULL DEFAULT '[]',
                    extracts jsonb NOT NULL DEFAULT '[]',
                    keys jsonb NOT NULL DEFAULT '[]',
                    created_by bigint,
                    started_at timestamptz NOT NULL DEFAULT now(),
                    finished_at timestamptz
                );
                CREATE INDEX idx_site_recipe_runs_site
                    ON site_recipe_runs (org_id, site, started_at DESC);
                CREATE INDEX idx_site_recipe_runs_recipe
                    ON site_recipe_runs (recipe_id, started_at DESC);",
            )
            .await?;
        Ok(())
    }

    async fn down(&self, m: &SchemaManager) -> Result<(), DbErr> {
        m.get_connection()
            .execute_unprepared(
                "DROP TABLE IF EXISTS site_recipe_runs; DROP TABLE IF EXISTS site_recipes;",
            )
            .await?;
        Ok(())
    }
}
