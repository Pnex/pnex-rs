//! A declared identity column read through a conversion (pipelines.md D569,
//! gap 3): the mapping field that converted it, so that erasure converts the
//! cell the same way before comparing it with the stored value.

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, m: &SchemaManager) -> Result<(), DbErr> {
        m.get_connection()
            .execute_unprepared("ALTER TABLE dataset_identity_columns ADD COLUMN spec jsonb;")
            .await?;
        Ok(())
    }

    async fn down(&self, m: &SchemaManager) -> Result<(), DbErr> {
        m.get_connection()
            .execute_unprepared("ALTER TABLE dataset_identity_columns DROP COLUMN spec;")
            .await?;
        Ok(())
    }
}
