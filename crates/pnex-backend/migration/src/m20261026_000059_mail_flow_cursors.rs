//! Where each `mail_received` node of a flow stands (mail.md D464): the
//! last mail it emitted, so a node that starts again (deploy, restart, a
//! lost bus) replays the stored mails it missed, never one twice.

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, m: &SchemaManager) -> Result<(), DbErr> {
        m.get_connection()
            .execute_unprepared(
                "CREATE TABLE mail_flow_cursors (
                    flow_id bigint NOT NULL REFERENCES flows(id) ON DELETE CASCADE,
                    node_id varchar(128) NOT NULL,
                    org_id bigint NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
                    last_received_at timestamptz NOT NULL,
                    last_id uuid,
                    updated_at timestamptz NOT NULL DEFAULT now(),
                    PRIMARY KEY (flow_id, node_id)
                );",
            )
            .await?;
        Ok(())
    }

    async fn down(&self, m: &SchemaManager) -> Result<(), DbErr> {
        m.get_connection()
            .execute_unprepared("DROP TABLE IF EXISTS mail_flow_cursors;")
            .await?;
        Ok(())
    }
}
