//! The daily session summary of an org (markets-research.md D814): one row
//! per org and session claims the summary, so it is written and sent once.

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, m: &SchemaManager) -> Result<(), DbErr> {
        m.get_connection()
            .execute_unprepared(
                "CREATE TABLE market_session_summaries (
                   org_id bigint NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
                   session date NOT NULL,
                   state text NOT NULL,
                   created_at timestamptz NOT NULL DEFAULT now(),
                   PRIMARY KEY (org_id, session)
                 );",
            )
            .await?;
        Ok(())
    }

    async fn down(&self, m: &SchemaManager) -> Result<(), DbErr> {
        m.get_connection()
            .execute_unprepared("DROP TABLE market_session_summaries;")
            .await?;
        Ok(())
    }
}
