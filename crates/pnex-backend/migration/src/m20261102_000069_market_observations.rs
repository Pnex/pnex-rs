//! Dated values of an object, with the moment each became known
//! (markets-research.md D541): a restated figure adds a row, it never
//! replaces the one a past decision read.

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, m: &SchemaManager) -> Result<(), DbErr> {
        m.get_connection()
            .execute_unprepared(
                "CREATE TABLE market_observations (
                   org_id bigint NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
                   object_id uuid NOT NULL REFERENCES objects(id) ON DELETE CASCADE,
                   metric text NOT NULL,
                   unit text NOT NULL DEFAULT '',
                   period_start date NOT NULL,
                   period_end date NOT NULL,
                   known_at timestamptz NOT NULL,
                   value double precision NOT NULL,
                   source_ref text NOT NULL,
                   backfilled boolean NOT NULL DEFAULT false,
                   PRIMARY KEY (org_id, object_id, metric, unit, period_start, period_end, known_at)
                 );",
            )
            .await?;
        Ok(())
    }

    async fn down(&self, m: &SchemaManager) -> Result<(), DbErr> {
        m.get_connection()
            .execute_unprepared("DROP TABLE IF EXISTS market_observations;")
            .await?;
        Ok(())
    }
}
