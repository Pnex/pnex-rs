//! Daily counters of the notifications sent to recipients taken from flow
//! messages (partner-watch.md D313): per channel and per recipient (hashed
//! address), so that a flow never turns into a bulk mailer.

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, m: &SchemaManager) -> Result<(), DbErr> {
        m.get_connection()
            .execute_unprepared(
                "CREATE TABLE notify_dynamic_sends (
                    org_id bigint NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
                    channel_id uuid NOT NULL REFERENCES notify_channels(id) ON DELETE CASCADE,
                    day date NOT NULL,
                    recipient_hash varchar(64) NOT NULL,
                    count integer NOT NULL DEFAULT 0,
                    PRIMARY KEY (channel_id, day, recipient_hash)
                );",
            )
            .await?;
        Ok(())
    }

    async fn down(&self, m: &SchemaManager) -> Result<(), DbErr> {
        m.get_connection()
            .execute_unprepared("DROP TABLE IF EXISTS notify_dynamic_sends;")
            .await?;
        Ok(())
    }
}
