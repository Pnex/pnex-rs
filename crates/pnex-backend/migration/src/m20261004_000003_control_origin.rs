//! Surface-declared controls (D131): a dashboard control widget or an
//! annotation `control` item provisions its own control at save. `origin`
//! records the declaring item (`{kind}:{surface_uuid}:{item_id}`); NULL is
//! a standalone control. One control per declaring item and org (unique
//! index; NULLs never collide on either engine). Additive: one nullable
//! column and one index.

use sea_orm_migration::prelude::*;
use sea_orm_migration::sea_orm::DatabaseBackend;

const POSTGRES_UP: &str = r#"
ALTER TABLE controls ADD COLUMN origin character varying(255);
CREATE UNIQUE INDEX uniq_controls_org_origin ON controls USING btree (org_id, origin);
"#;

const SQLITE_UP: &str = r#"
ALTER TABLE controls ADD COLUMN origin varchar(255) NULL;
CREATE UNIQUE INDEX uniq_controls_org_origin ON controls (org_id, origin);
"#;

const POSTGRES_DOWN: &str = r#"
DROP INDEX IF EXISTS uniq_controls_org_origin;
ALTER TABLE controls DROP COLUMN IF EXISTS origin;
"#;

const SQLITE_DOWN: &str = r#"
DROP INDEX IF EXISTS uniq_controls_org_origin;
ALTER TABLE controls DROP COLUMN origin;
"#;

#[derive(DeriveMigrationName)]
pub struct Migration;

fn script(
    m: &SchemaManager,
    pg: &'static str,
    sqlite: &'static str,
) -> Result<&'static str, DbErr> {
    match m.get_database_backend() {
        DatabaseBackend::Postgres => Ok(pg),
        DatabaseBackend::Sqlite => Ok(sqlite),
        other => Err(DbErr::Migration(format!(
            "unsupported database backend: {other:?}"
        ))),
    }
}

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, m: &SchemaManager) -> Result<(), DbErr> {
        let sql = script(m, POSTGRES_UP, SQLITE_UP)?;
        m.get_connection().execute_unprepared(sql).await?;
        Ok(())
    }

    async fn down(&self, m: &SchemaManager) -> Result<(), DbErr> {
        let sql = script(m, POSTGRES_DOWN, SQLITE_DOWN)?;
        m.get_connection().execute_unprepared(sql).await?;
        Ok(())
    }
}
