//! Org controls (D125): the write side of the surfaces. A dashboard card or
//! an annotation writes the value of a control (stored in Valkey, never in
//! the database); deployed flows listen through the `control-source` node.
//! Additive: one new table.

use sea_orm_migration::prelude::*;
use sea_orm_migration::sea_orm::DatabaseBackend;

const POSTGRES_UP: &str = r#"
CREATE TABLE controls (
    id uuid DEFAULT gen_random_uuid() NOT NULL,
    org_id bigint NOT NULL,
    key character varying(64) NOT NULL,
    label character varying(255) NOT NULL,
    kind character varying(16) NOT NULL,
    spec jsonb NOT NULL,
    created_by bigint,
    created_at timestamp with time zone DEFAULT CURRENT_TIMESTAMP NOT NULL,
    updated_at timestamp with time zone DEFAULT CURRENT_TIMESTAMP NOT NULL,
    CONSTRAINT controls_pkey PRIMARY KEY (id),
    CONSTRAINT "fk-controls-org_id-to-organizations" FOREIGN KEY (org_id)
        REFERENCES organizations(id) ON DELETE CASCADE
);
CREATE UNIQUE INDEX uniq_controls_org_key ON controls USING btree (org_id, key);
"#;

const SQLITE_UP: &str = r#"
CREATE TABLE "controls" ( "id" uuid_text NOT NULL PRIMARY KEY, "org_id" integer NOT NULL, "key" varchar(64) NOT NULL, "label" varchar(255) NOT NULL, "kind" varchar(16) NOT NULL, "spec" jsonb_text NOT NULL, "created_by" integer NULL, "created_at" timestamp_with_timezone_text NOT NULL DEFAULT CURRENT_TIMESTAMP, "updated_at" timestamp_with_timezone_text NOT NULL DEFAULT CURRENT_TIMESTAMP, FOREIGN KEY ("org_id") REFERENCES "organizations" ("id") ON DELETE CASCADE );
CREATE UNIQUE INDEX uniq_controls_org_key ON controls (org_id, key);
"#;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, m: &SchemaManager) -> Result<(), DbErr> {
        let script = match m.get_database_backend() {
            DatabaseBackend::Postgres => POSTGRES_UP,
            DatabaseBackend::Sqlite => SQLITE_UP,
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
        m.get_connection()
            .execute_unprepared("DROP TABLE IF EXISTS controls")
            .await?;
        Ok(())
    }
}
