//! Access management, lot I6 (iam.md §7): service accounts of an org and
//! their access keys. A service account is a `users` row marked
//! `is_service` (authors and audit reference an identity, D299) with no
//! membership: its rights are its assignments only. A key is stored as its
//! sha256, never in clear (R15, R16, R23).

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, m: &SchemaManager) -> Result<(), DbErr> {
        m.get_connection()
            .execute_unprepared(
                "ALTER TABLE users ADD COLUMN is_service boolean NOT NULL DEFAULT false;
                CREATE TABLE iam_service_accounts (
                    id uuid PRIMARY KEY,
                    org_id bigint NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
                    user_id bigint NOT NULL UNIQUE REFERENCES users(id) ON DELETE CASCADE,
                    name varchar(120) NOT NULL,
                    description text NOT NULL DEFAULT '',
                    created_by bigint REFERENCES users(id) ON DELETE SET NULL,
                    created_at timestamptz NOT NULL DEFAULT now(),
                    UNIQUE (org_id, name)
                );
                CREATE TABLE iam_access_keys (
                    id uuid PRIMARY KEY,
                    service_account_id uuid NOT NULL REFERENCES iam_service_accounts(id) ON DELETE CASCADE,
                    prefix varchar(24) NOT NULL,
                    secret_sha256 char(64) NOT NULL UNIQUE,
                    expires_at timestamptz NOT NULL,
                    created_by bigint REFERENCES users(id) ON DELETE SET NULL,
                    created_at timestamptz NOT NULL DEFAULT now(),
                    last_used_at timestamptz,
                    revoked_at timestamptz
                );
                CREATE INDEX iam_access_keys_account ON iam_access_keys (service_account_id);",
            )
            .await?;
        Ok(())
    }

    async fn down(&self, m: &SchemaManager) -> Result<(), DbErr> {
        m.get_connection()
            .execute_unprepared(
                "DROP TABLE IF EXISTS iam_access_keys;
                 DROP TABLE IF EXISTS iam_service_accounts;
                 DELETE FROM users WHERE is_service;
                 ALTER TABLE users DROP COLUMN IF EXISTS is_service;",
            )
            .await?;
        Ok(())
    }
}
