//! A person may file a proposal (parts-docs.md PD1d): `author_id` is who filed it, and
//! an author never accepts their own. `links` are the extra links an `object_create`
//! makes on acceptance (a field note's photos), `[{"link_type", "target_id"}]`.

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, m: &SchemaManager) -> Result<(), DbErr> {
        m.get_connection()
            .execute_unprepared(
                "ALTER TABLE proposals \
                   ADD COLUMN IF NOT EXISTS author_id bigint REFERENCES users(id) ON DELETE SET NULL, \
                   ADD COLUMN IF NOT EXISTS links jsonb;",
            )
            .await?;
        Ok(())
    }

    async fn down(&self, m: &SchemaManager) -> Result<(), DbErr> {
        m.get_connection()
            .execute_unprepared(
                "ALTER TABLE proposals DROP COLUMN IF EXISTS author_id, DROP COLUMN IF EXISTS links;",
            )
            .await?;
        Ok(())
    }
}
