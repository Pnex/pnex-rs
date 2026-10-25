//! Applications declared in an org (external-apps.md D433): an external
//! program dressed around a service account, with the scope it reads.

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, m: &SchemaManager) -> Result<(), DbErr> {
        m.get_connection()
            .execute_unprepared(
                "CREATE TABLE apps (
                    id uuid PRIMARY KEY,
                    org_id bigint NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
                    key varchar(64) NOT NULL,
                    name varchar(120) NOT NULL,
                    description text NOT NULL DEFAULT '',
                    service_account_id uuid UNIQUE REFERENCES iam_service_accounts(id) ON DELETE SET NULL,
                    scope jsonb NOT NULL DEFAULT '{}'::jsonb,
                    version integer NOT NULL DEFAULT 1,
                    created_by bigint REFERENCES users(id) ON DELETE SET NULL,
                    created_at timestamptz NOT NULL DEFAULT now(),
                    updated_at timestamptz NOT NULL DEFAULT now(),
                    UNIQUE (org_id, key)
                );",
            )
            .await?;
        Ok(())
    }

    async fn down(&self, m: &SchemaManager) -> Result<(), DbErr> {
        m.get_connection()
            .execute_unprepared("DROP TABLE IF EXISTS apps;")
            .await?;
        Ok(())
    }
}
