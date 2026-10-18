//! Mode of a browser agent (extension-collector.md D269): `shared` runs
//! every platform but the exclusive ones (TikTok pauses in a hidden tab),
//! `dedicated:<platform>` runs that platform only.

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, m: &SchemaManager) -> Result<(), DbErr> {
        m.get_connection()
            .execute_unprepared(
                "ALTER TABLE extension_agents ADD COLUMN mode varchar(32) NOT NULL DEFAULT 'shared';",
            )
            .await?;
        Ok(())
    }

    async fn down(&self, m: &SchemaManager) -> Result<(), DbErr> {
        m.get_connection()
            .execute_unprepared("ALTER TABLE extension_agents DROP COLUMN IF EXISTS mode;")
            .await?;
        Ok(())
    }
}
