//! A member's own network connections shared with a team org (crm.md
//! D999): one switch per org, member and network, and the person of the
//! team org that stands for the member. Off by default (no row).

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, m: &SchemaManager) -> Result<(), DbErr> {
        m.get_connection()
            .execute_unprepared(
                "CREATE TABLE connection_shares (
                   org_id bigint NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
                   user_id bigint NOT NULL REFERENCES users(id) ON DELETE CASCADE,
                   network varchar(32) NOT NULL,
                   shared boolean NOT NULL,
                   person_id uuid REFERENCES objects(id) ON DELETE SET NULL,
                   updated_at timestamptz NOT NULL DEFAULT now(),
                   PRIMARY KEY (org_id, user_id, network)
                 );
                 CREATE INDEX connection_shares_user ON connection_shares (user_id, network) WHERE shared;",
            )
            .await?;
        Ok(())
    }

    async fn down(&self, m: &SchemaManager) -> Result<(), DbErr> {
        m.get_connection()
            .execute_unprepared("DROP TABLE connection_shares;")
            .await?;
        Ok(())
    }
}
