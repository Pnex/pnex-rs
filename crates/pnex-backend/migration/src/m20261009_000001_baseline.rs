//! Baseline schema of 0.1.0 (cut 2026-10-09): every pre-release migration
//! collapsed into one plain-SQL script per backend, with the pre-vault
//! leftovers removed (`ai_connectors`, plaintext
//! `wifi_credentials.wifi_password`) and the compatibility columns
//! dropped. PostgreSQL only (decision #19).

use sea_orm_migration::prelude::*;
use sea_orm_migration::sea_orm::DatabaseBackend;

const POSTGRES: &str = include_str!("baseline/postgres.sql");
/// Tables of the baseline, in creation order (`down` drops them reversed).
const TABLES: &str = include_str!("baseline/tables.txt");
/// PostgreSQL enum types of the baseline.
const PG_TYPES: [&str; 8] = [
    "capability_mode",
    "constant_kind",
    "conversion_kind",
    "data_source_kind",
    "formula_kind",
    "openobserve_org_status",
    "org_member_role",
    "ui_theme",
];

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, m: &SchemaManager) -> Result<(), DbErr> {
        let script = match m.get_database_backend() {
            DatabaseBackend::Postgres => POSTGRES,
            other => {
                return Err(DbErr::Migration(format!(
                    "unsupported database backend: {other:?}"
                )))
            }
        };
        m.get_connection().execute_unprepared(script).await?;
        Ok(())
    }

    async fn down(&self, m: &SchemaManager) -> Result<(), DbErr> {
        let db = m.get_connection();
        let tables: Vec<&str> = TABLES.lines().filter(|l| !l.is_empty()).rev().collect();
        db.execute_unprepared(&format!(
            "DROP TABLE IF EXISTS {} CASCADE",
            tables.join(", ")
        ))
        .await?;
        db.execute_unprepared(&format!(
            "DROP TYPE IF EXISTS {} CASCADE",
            PG_TYPES.join(", ")
        ))
        .await?;
        Ok(())
    }
}
