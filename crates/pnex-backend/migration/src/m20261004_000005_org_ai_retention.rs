//! Assistant conversation retention per org (D145): an org may only keep
//! its conversations for LESS time than the platform value; NULL follows
//! the platform. Additive: one nullable column.

use sea_orm_migration::prelude::*;
use sea_orm_migration::sea_orm::DatabaseBackend;

#[derive(DeriveMigrationName)]
pub struct Migration;

fn check(m: &SchemaManager) -> Result<(), DbErr> {
    match m.get_database_backend() {
        DatabaseBackend::Postgres | DatabaseBackend::Sqlite => Ok(()),
        other => Err(DbErr::Migration(format!(
            "unsupported database backend: {other:?}"
        ))),
    }
}

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, m: &SchemaManager) -> Result<(), DbErr> {
        check(m)?;
        // Same statement on both engines.
        m.get_connection()
            .execute_unprepared("ALTER TABLE organizations ADD COLUMN ai_retention_days integer;")
            .await?;
        Ok(())
    }

    async fn down(&self, m: &SchemaManager) -> Result<(), DbErr> {
        check(m)?;
        m.get_connection()
            .execute_unprepared("ALTER TABLE organizations DROP COLUMN ai_retention_days;")
            .await?;
        Ok(())
    }
}
