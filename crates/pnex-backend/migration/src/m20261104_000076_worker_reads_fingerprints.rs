//! The worker leaves erased people out of what it writes (data-pipelines.md D569):
//! it reads their fingerprints, never a value.

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, m: &SchemaManager) -> Result<(), DbErr> {
        m.get_connection()
            .execute_unprepared(
                "GRANT SELECT (dataset_id, column_name, value) ON dataset_erasures
                   TO pnex_pipeline_worker;",
            )
            .await?;
        Ok(())
    }

    async fn down(&self, m: &SchemaManager) -> Result<(), DbErr> {
        m.get_connection()
            .execute_unprepared(
                "REVOKE SELECT (dataset_id, column_name, value) ON dataset_erasures
                   FROM pnex_pipeline_worker;",
            )
            .await?;
        Ok(())
    }
}
