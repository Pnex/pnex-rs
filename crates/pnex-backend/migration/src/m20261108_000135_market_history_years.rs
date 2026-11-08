//! Yearly partitions of the price history (markets-research.md MK11 bis):
//! HistData publishes a closed year as one file.

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, m: &SchemaManager) -> Result<(), DbErr> {
        m.get_connection()
            .execute_unprepared(
                "ALTER TABLE market_history_partitions DROP CONSTRAINT market_history_partitions_period_check;
                 ALTER TABLE market_history_partitions ADD CONSTRAINT market_history_partitions_period_check
                   CHECK (period IN ('year', 'month', 'day'));",
            )
            .await?;
        Ok(())
    }

    async fn down(&self, m: &SchemaManager) -> Result<(), DbErr> {
        m.get_connection()
            .execute_unprepared(
                "DELETE FROM market_history_partitions WHERE period = 'year';
                 ALTER TABLE market_history_partitions DROP CONSTRAINT market_history_partitions_period_check;
                 ALTER TABLE market_history_partitions ADD CONSTRAINT market_history_partitions_period_check
                   CHECK (period IN ('month', 'day'));",
            )
            .await?;
        Ok(())
    }
}
