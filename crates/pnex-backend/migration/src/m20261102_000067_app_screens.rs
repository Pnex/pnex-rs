//! Screens of an application (app-builder.md D507, D509, D440): the screens
//! are dashboards owned by their application, never listed as dashboards.

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, m: &SchemaManager) -> Result<(), DbErr> {
        m.get_connection()
            .execute_unprepared(
                "ALTER TABLE apps
                   ADD COLUMN names jsonb NOT NULL DEFAULT '{}'::jsonb,
                   ADD COLUMN screens jsonb NOT NULL DEFAULT '[]'::jsonb,
                   ADD COLUMN home text,
                   ADD COLUMN variables jsonb NOT NULL DEFAULT '[]'::jsonb,
                   ADD COLUMN actions jsonb NOT NULL DEFAULT '[]'::jsonb;
                 ALTER TABLE dashboards
                   ADD COLUMN app_id uuid REFERENCES apps (id) ON DELETE CASCADE;
                 CREATE INDEX dashboards_app_id_idx ON dashboards (app_id)
                   WHERE app_id IS NOT NULL;",
            )
            .await?;
        Ok(())
    }

    async fn down(&self, m: &SchemaManager) -> Result<(), DbErr> {
        m.get_connection()
            .execute_unprepared(
                "ALTER TABLE dashboards DROP COLUMN IF EXISTS app_id;
                 ALTER TABLE apps
                   DROP COLUMN IF EXISTS names,
                   DROP COLUMN IF EXISTS screens,
                   DROP COLUMN IF EXISTS home,
                   DROP COLUMN IF EXISTS variables,
                   DROP COLUMN IF EXISTS actions;",
            )
            .await?;
        Ok(())
    }
}
