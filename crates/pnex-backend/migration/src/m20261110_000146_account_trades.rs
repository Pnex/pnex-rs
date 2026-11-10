//! Public trading accounts (markets-research.md appendix K, D927): their
//! trades are rows keyed by account and trade, never objects. A re-imported
//! trade keeps its first `known_at`; deleting the account purges its rows
//! (D932). PostgreSQL rows rather than a dataset, Shan's choice
//! (2026-11-10): dedup and purge are one statement each.

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, m: &SchemaManager) -> Result<(), DbErr> {
        m.get_connection()
            .execute_unprepared(
                "CREATE TABLE account_trades (
                   org_id bigint NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
                   platform text NOT NULL,
                   account_ref text NOT NULL,
                   trade_key text NOT NULL,
                   symbol text NOT NULL,
                   side text NOT NULL,
                   lots double precision NOT NULL,
                   open_at timestamptz NOT NULL,
                   close_at timestamptz,
                   open_price double precision NOT NULL,
                   close_price double precision,
                   sl double precision,
                   tp double precision,
                   profit double precision,
                   pips double precision,
                   commission double precision,
                   swap double precision,
                   gain_pct double precision,
                   magic text,
                   comment text,
                   source text NOT NULL,
                   known_at timestamptz NOT NULL,
                   backfilled boolean NOT NULL,
                   PRIMARY KEY (org_id, platform, account_ref, trade_key)
                 );
                 CREATE INDEX account_trades_symbol ON account_trades (org_id, symbol, open_at);",
            )
            .await?;
        Ok(())
    }

    async fn down(&self, m: &SchemaManager) -> Result<(), DbErr> {
        m.get_connection()
            .execute_unprepared("DROP TABLE IF EXISTS account_trades;")
            .await?;
        Ok(())
    }
}
