//! Pending invitations to an org (SEC-72): a membership exists only once the
//! invited person accepts. One pending row per (org, email); accepting,
//! declining or revoking deletes it, an expired row is ignored and replaced
//! by the next invitation.

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, m: &SchemaManager) -> Result<(), DbErr> {
        m.get_connection()
            .execute_unprepared(
                "CREATE TABLE org_invitations (
                   id bigserial PRIMARY KEY,
                   org_id bigint NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
                   email text NOT NULL CHECK (email = lower(email)),
                   role text NOT NULL CHECK (role IN ('owner', 'admin', 'member', 'viewer')),
                   invited_by bigint REFERENCES users(id) ON DELETE SET NULL,
                   created_at timestamptz NOT NULL DEFAULT now(),
                   expires_at timestamptz NOT NULL,
                   UNIQUE (org_id, email)
                 );
                 CREATE INDEX org_invitations_email ON org_invitations (email);",
            )
            .await?;
        Ok(())
    }

    async fn down(&self, m: &SchemaManager) -> Result<(), DbErr> {
        m.get_connection()
            .execute_unprepared("DROP TABLE org_invitations;")
            .await?;
        Ok(())
    }
}
