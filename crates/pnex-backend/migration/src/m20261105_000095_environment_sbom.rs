//! The checksum of the SBOM of a scan (agent-environments.md D728): the document lives in
//! OpenObserve, and this says it was stored and what it must read back as.

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, m: &SchemaManager) -> Result<(), DbErr> {
        m.get_connection()
            .execute_unprepared(
                "ALTER TABLE environment_scans ADD COLUMN IF NOT EXISTS sbom_sha256 text;",
            )
            .await?;
        Ok(())
    }

    async fn down(&self, m: &SchemaManager) -> Result<(), DbErr> {
        m.get_connection()
            .execute_unprepared("ALTER TABLE environment_scans DROP COLUMN sbom_sha256;")
            .await?;
        Ok(())
    }
}
