//! The partitions of a price history source (markets-research.md D808): one
//! row per (org, source, instrument, period). A `done` partition is never
//! fetched again, so a run cut off goes on where it stopped.

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, m: &SchemaManager) -> Result<(), DbErr> {
        m.get_connection()
            .execute_unprepared(
                "CREATE TABLE market_history_partitions (
                   org_id bigint NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
                   source text NOT NULL,
                   instrument text NOT NULL,
                   period text NOT NULL CHECK (period IN ('month', 'day')),
                   period_start date NOT NULL,
                   state text NOT NULL CHECK (state IN ('done', 'empty', 'failed')),
                   code text,
                   rows bigint NOT NULL DEFAULT 0,
                   bytes bigint NOT NULL DEFAULT 0,
                   sha256 text,
                   transaction_id uuid,
                   fetched_at timestamptz NOT NULL DEFAULT now(),
                   PRIMARY KEY (org_id, source, instrument, period, period_start)
                 );",
            )
            .await?;
        Ok(())
    }

    async fn down(&self, m: &SchemaManager) -> Result<(), DbErr> {
        m.get_connection()
            .execute_unprepared("DROP TABLE market_history_partitions;")
            .await?;
        Ok(())
    }
}
