//! Profile and employee list captures are default navigation rules now
//! (agent-unification.md D780, lot U4): the missions of those two kinds go.
//! Beta, no compatibility kept; the documents they captured stay.

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, m: &SchemaManager) -> Result<(), DbErr> {
        m.get_connection()
            .execute_unprepared(
                "DELETE FROM extension_missions WHERE kind IN ('capture_profile', 'capture_people');",
            )
            .await?;
        Ok(())
    }

    async fn down(&self, _m: &SchemaManager) -> Result<(), DbErr> {
        // The deleted missions are not recreated: their kinds no longer exist.
        Ok(())
    }
}
