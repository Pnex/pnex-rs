//! Point-in-time access keys (markets-research.md D898): a key whose every
//! read must carry `as_of`, so a backtest never reads the present by default.

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, m: &SchemaManager) -> Result<(), DbErr> {
        m.get_connection()
            .execute_unprepared(
                "ALTER TABLE iam_access_keys ADD COLUMN point_in_time boolean NOT NULL DEFAULT false;",
            )
            .await?;
        Ok(())
    }

    async fn down(&self, m: &SchemaManager) -> Result<(), DbErr> {
        m.get_connection()
            .execute_unprepared("ALTER TABLE iam_access_keys DROP COLUMN point_in_time;")
            .await?;
        Ok(())
    }
}
