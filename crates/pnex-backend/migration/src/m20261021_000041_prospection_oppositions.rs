//! Prospecting opposition list (account-mapping.md D341, minimum): one
//! fingerprint per row, `person:<uuid>` or a canonical profile address.
//! Profile captures (D333) read it and refuse; it outlives the person.

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, m: &SchemaManager) -> Result<(), DbErr> {
        m.get_connection()
            .execute_unprepared(
                "CREATE TABLE prospection_oppositions (
                    org_id bigint NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
                    fingerprint varchar(2048) NOT NULL,
                    created_by bigint REFERENCES users(id) ON DELETE SET NULL,
                    created_at timestamptz NOT NULL DEFAULT now(),
                    PRIMARY KEY (org_id, fingerprint)
                );",
            )
            .await?;
        Ok(())
    }

    async fn down(&self, m: &SchemaManager) -> Result<(), DbErr> {
        m.get_connection()
            .execute_unprepared("DROP TABLE IF EXISTS prospection_oppositions;")
            .await?;
        Ok(())
    }
}
