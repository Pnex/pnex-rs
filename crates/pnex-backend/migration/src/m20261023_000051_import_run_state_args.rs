//! The arguments of a run's refusal or failure (`{rows, max}` for a source
//! too large, the missing columns…): History renders the message with them
//! (data-pipelines.md §7 bis, point 3).

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, m: &SchemaManager) -> Result<(), DbErr> {
        m.get_connection()
            .execute_unprepared("ALTER TABLE object_import_runs ADD COLUMN state_args jsonb;")
            .await?;
        Ok(())
    }

    async fn down(&self, m: &SchemaManager) -> Result<(), DbErr> {
        m.get_connection()
            .execute_unprepared("ALTER TABLE object_import_runs DROP COLUMN IF EXISTS state_args;")
            .await?;
        Ok(())
    }
}
