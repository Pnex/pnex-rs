//! Prediction market prices, trades and read cursors leave PostgreSQL: the
//! pack's pipelines write them into the org's tables `prediction_prices` and
//! `prediction_trades`, reloaded from zero (pipeline-engine.md D946, D952).

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, m: &SchemaManager) -> Result<(), DbErr> {
        m.get_connection()
            .execute_unprepared(
                "DROP TABLE prediction_prices; DROP TABLE prediction_trades; DROP TABLE prediction_cursors;",
            )
            .await?;
        Ok(())
    }

    async fn down(&self, m: &SchemaManager) -> Result<(), DbErr> {
        m.get_connection()
            .execute_unprepared(
                "CREATE TABLE prediction_prices (
                   org_id bigint NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
                   condition_id text NOT NULL,
                   token text NOT NULL,
                   outcome text NOT NULL,
                   fidelity integer NOT NULL,
                   ts timestamptz NOT NULL,
                   price double precision NOT NULL,
                   known_at timestamptz NOT NULL,
                   PRIMARY KEY (org_id, token, fidelity, ts)
                 );
                 CREATE INDEX prediction_prices_market ON prediction_prices (org_id, condition_id, ts);
                 CREATE TABLE prediction_trades (
                   org_id bigint NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
                   condition_id text NOT NULL,
                   token text NOT NULL,
                   outcome text NOT NULL,
                   wallet text NOT NULL,
                   side text NOT NULL,
                   size double precision NOT NULL,
                   price double precision NOT NULL,
                   ts timestamptz NOT NULL,
                   tx text NOT NULL,
                   known_at timestamptz NOT NULL DEFAULT now(),
                   PRIMARY KEY (org_id, tx, wallet, token, side, ts)
                 );
                 CREATE INDEX prediction_trades_market ON prediction_trades (org_id, condition_id, ts);
                 CREATE INDEX prediction_trades_wallet ON prediction_trades (org_id, wallet, ts);
                 CREATE TABLE prediction_cursors (
                   org_id bigint NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
                   key text NOT NULL,
                   last_ts bigint NOT NULL,
                   updated_at timestamptz NOT NULL DEFAULT now(),
                   PRIMARY KEY (org_id, key)
                 );",
            )
            .await?;
        Ok(())
    }
}
