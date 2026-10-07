//! Failure reason of a firmware build (O4): a machine code the UI
//! translates and an excerpt of the tool output, credentials masked by the
//! worker. Additive: two nullable columns.

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
        // Same statements on both engines (one per call: SQLite runs a
        // single statement).
        let db = m.get_connection();
        db.execute_unprepared("ALTER TABLE build_records ADD COLUMN failure_code text;")
            .await?;
        db.execute_unprepared("ALTER TABLE build_records ADD COLUMN failure_detail text;")
            .await?;
        Ok(())
    }

    async fn down(&self, m: &SchemaManager) -> Result<(), DbErr> {
        check(m)?;
        let db = m.get_connection();
        db.execute_unprepared("ALTER TABLE build_records DROP COLUMN failure_detail;")
            .await?;
        db.execute_unprepared("ALTER TABLE build_records DROP COLUMN failure_code;")
            .await?;
        Ok(())
    }
}
