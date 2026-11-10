//! HistData partitions were fetched on Binance's host (sweep bug fixed on
//! 2026-11-10): every one not `done` was recorded from a 404 and would
//! never be asked again. They are dropped so the next pass reads them.

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, m: &SchemaManager) -> Result<(), DbErr> {
        m.get_connection()
            .execute_unprepared(
                "DELETE FROM market_history_partitions WHERE source = 'histdata' AND state <> 'done';",
            )
            .await?;
        Ok(())
    }

    async fn down(&self, _m: &SchemaManager) -> Result<(), DbErr> {
        Ok(())
    }
}
