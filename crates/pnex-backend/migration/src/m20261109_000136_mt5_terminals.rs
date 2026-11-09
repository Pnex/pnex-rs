//! MetaTrader 5 terminals as registered data sources (markets-research.md
//! D847–D849): the registry with the hash of each terminal's token, the
//! catalogue each one announces, and the inbox of its calls, emptied into
//! the tables `mt5_ohlcv_1m` and `mt5_ticks` by the markets sweep.

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, m: &SchemaManager) -> Result<(), DbErr> {
        m.get_connection()
            .execute_unprepared(
                "CREATE TABLE mt5_terminals (
                   id uuid PRIMARY KEY,
                   org_id bigint NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
                   name text NOT NULL,
                   broker text NOT NULL DEFAULT '',
                   server text NOT NULL DEFAULT '',
                   account text NOT NULL DEFAULT '',
                   bars text NOT NULL DEFAULT '*',
                   ticks text NOT NULL DEFAULT '',
                   token_hash text,
                   ea_version text,
                   terminal_build bigint,
                   last_seen_at timestamptz,
                   backfill_days integer NOT NULL DEFAULT 0,
                   backfill_requested_at timestamptz,
                   version integer NOT NULL DEFAULT 1,
                   created_at timestamptz NOT NULL DEFAULT now(),
                   updated_at timestamptz NOT NULL DEFAULT now(),
                   UNIQUE (org_id, name)
                 );
                 CREATE UNIQUE INDEX mt5_terminals_token ON mt5_terminals (token_hash)
                   WHERE token_hash IS NOT NULL;
                 CREATE TABLE mt5_symbols (
                   terminal_id uuid NOT NULL REFERENCES mt5_terminals(id) ON DELETE CASCADE,
                   symbol text NOT NULL,
                   description text NOT NULL DEFAULT '',
                   path text NOT NULL DEFAULT '',
                   base text NOT NULL DEFAULT '',
                   profit text NOT NULL DEFAULT '',
                   digits integer NOT NULL DEFAULT 0,
                   contract_size double precision NOT NULL DEFAULT 0,
                   first_m1 timestamptz,
                   announced_at timestamptz NOT NULL DEFAULT now(),
                   PRIMARY KEY (terminal_id, symbol)
                 );
                 CREATE TABLE mt5_inbox (
                   id bigserial PRIMARY KEY,
                   org_id bigint NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
                   terminal_id uuid NOT NULL REFERENCES mt5_terminals(id) ON DELETE CASCADE,
                   body jsonb NOT NULL,
                   received_at timestamptz NOT NULL DEFAULT now()
                 );
                 CREATE INDEX mt5_inbox_org ON mt5_inbox (org_id, id);
                 CREATE INDEX mt5_inbox_terminal ON mt5_inbox (terminal_id);",
            )
            .await?;
        Ok(())
    }

    async fn down(&self, m: &SchemaManager) -> Result<(), DbErr> {
        m.get_connection()
            .execute_unprepared(
                "DROP TABLE mt5_inbox; DROP TABLE mt5_symbols; DROP TABLE mt5_terminals;",
            )
            .await?;
        Ok(())
    }
}
