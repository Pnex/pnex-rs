//! Scan policy of an org and the count of a scan against it (agent-environments.md D748):
//! a version over the threshold is signaled by default, refused in `refuse` mode.

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, m: &SchemaManager) -> Result<(), DbErr> {
        m.get_connection()
            .execute_unprepared(
                "CREATE TABLE environment_scan_policies (
                   org_id bigint PRIMARY KEY REFERENCES organizations(id) ON DELETE CASCADE,
                   threshold text NOT NULL CHECK (threshold IN ('critical', 'high', 'none')),
                   mode text NOT NULL CHECK (mode IN ('signal', 'refuse')),
                   updated_by bigint REFERENCES users(id) ON DELETE SET NULL,
                   updated_at timestamptz NOT NULL DEFAULT now()
                 );
                 ALTER TABLE environment_scans ADD COLUMN breaches integer;",
            )
            .await?;
        Ok(())
    }

    async fn down(&self, m: &SchemaManager) -> Result<(), DbErr> {
        m.get_connection()
            .execute_unprepared(
                "ALTER TABLE environment_scans DROP COLUMN breaches;
                 DROP TABLE environment_scan_policies;",
            )
            .await?;
        Ok(())
    }
}
