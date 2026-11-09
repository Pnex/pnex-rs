//! Decisions of an external model (markets-research.md D894): one per
//! (model, version, instrument, interval, `as_of`), posted by a service
//! account, read and measured by PNeX. Advice only: nothing here orders.

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, m: &SchemaManager) -> Result<(), DbErr> {
        m.get_connection()
            .execute_unprepared(
                "CREATE TABLE market_decisions (
                   id uuid PRIMARY KEY,
                   org_id bigint NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
                   model text NOT NULL,
                   model_version text NOT NULL,
                   instrument text NOT NULL,
                   source text NOT NULL,
                   interval text NOT NULL,
                   horizon_bars integer NOT NULL,
                   as_of timestamptz NOT NULL,
                   probabilities double precision[] NOT NULL,
                   best text NOT NULL,
                   confidence double precision NOT NULL,
                   barriers jsonb,
                   inputs jsonb,
                   service_account_id uuid,
                   created_at timestamptz NOT NULL DEFAULT now(),
                   UNIQUE (org_id, model, model_version, instrument, interval, as_of)
                 );
                 CREATE INDEX market_decisions_list ON market_decisions (org_id, as_of DESC);
                 CREATE INDEX market_decisions_day ON market_decisions (org_id, created_at);",
            )
            .await?;
        Ok(())
    }

    async fn down(&self, m: &SchemaManager) -> Result<(), DbErr> {
        m.get_connection()
            .execute_unprepared("DROP TABLE market_decisions;")
            .await?;
        Ok(())
    }
}
