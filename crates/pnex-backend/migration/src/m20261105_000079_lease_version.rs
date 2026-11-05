//! Who holds a lease, in which version and since when (data-pipelines.md §7 bis,
//! point 2): an old pod holding a sweep was invisible during a partial rollout.

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, m: &SchemaManager) -> Result<(), DbErr> {
        m.get_connection()
            .execute_unprepared(
                "ALTER TABLE flow_leases
                   ADD COLUMN holder_version text NOT NULL DEFAULT '',
                   ADD COLUMN taken_at timestamptz NOT NULL DEFAULT now();",
            )
            .await?;
        Ok(())
    }

    async fn down(&self, m: &SchemaManager) -> Result<(), DbErr> {
        m.get_connection()
            .execute_unprepared(
                "ALTER TABLE flow_leases DROP COLUMN holder_version, DROP COLUMN taken_at;",
            )
            .await?;
        Ok(())
    }
}
