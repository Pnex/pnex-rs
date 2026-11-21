//! End of the pipeline engine rework (pipelines.md PE9): the market history
//! partitions and the per-object cursor of imports are read by nothing since
//! the price tables and the pipelines' own rows replaced them.

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, m: &SchemaManager) -> Result<(), DbErr> {
        let db = m.get_connection();
        db.execute_unprepared("DROP TABLE IF EXISTS market_history_partitions;")
            .await?;
        db.execute_unprepared(
            "ALTER TABLE object_imports DROP COLUMN IF EXISTS per_object_cursor;",
        )
        .await?;
        Ok(())
    }

    async fn down(&self, _m: &SchemaManager) -> Result<(), DbErr> {
        // Dropped for good: what they held came from sources read again.
        Ok(())
    }
}
