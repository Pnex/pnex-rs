//! A trading account's owner may hide position sizes on the platform
//! (markets-research.md D927): `lots` becomes optional, as `sl` and `tp`.

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, m: &SchemaManager) -> Result<(), DbErr> {
        m.get_connection()
            .execute_unprepared("ALTER TABLE account_trades ALTER COLUMN lots DROP NOT NULL;")
            .await?;
        Ok(())
    }

    async fn down(&self, m: &SchemaManager) -> Result<(), DbErr> {
        m.get_connection()
            .execute_unprepared(
                "DELETE FROM account_trades WHERE lots IS NULL; \
                 ALTER TABLE account_trades ALTER COLUMN lots SET NOT NULL;",
            )
            .await?;
        Ok(())
    }
}
