//! Versions of the site navigation rules (site-recipes.md D323, D221):
//! every save keeps its YAML; the studio opens, compares and restores them.

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, m: &SchemaManager) -> Result<(), DbErr> {
        m.get_connection()
            .execute_unprepared(
                "CREATE TABLE site_recipe_versions (
                    recipe_id uuid NOT NULL REFERENCES site_recipes(id) ON DELETE CASCADE,
                    version integer NOT NULL,
                    yaml text NOT NULL,
                    created_by bigint REFERENCES users(id) ON DELETE SET NULL,
                    created_at timestamptz NOT NULL DEFAULT now(),
                    PRIMARY KEY (recipe_id, version)
                );
                INSERT INTO site_recipe_versions (recipe_id, version, yaml, created_by, created_at)
                    SELECT id, version, yaml, updated_by, updated_at FROM site_recipes;",
            )
            .await?;
        Ok(())
    }

    async fn down(&self, m: &SchemaManager) -> Result<(), DbErr> {
        m.get_connection()
            .execute_unprepared("DROP TABLE IF EXISTS site_recipe_versions;")
            .await?;
        Ok(())
    }
}
