//! The meeting invitation a mail carries (mail.md D466): its first event,
//! read at polling, kept with the mail so a reader can add it to the
//! calendar without fetching the mail again.

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, m: &SchemaManager) -> Result<(), DbErr> {
        m.get_connection()
            .execute_unprepared("ALTER TABLE mail_messages ADD COLUMN invitation jsonb;")
            .await?;
        Ok(())
    }

    async fn down(&self, m: &SchemaManager) -> Result<(), DbErr> {
        m.get_connection()
            .execute_unprepared("ALTER TABLE mail_messages DROP COLUMN IF EXISTS invitation;")
            .await?;
        Ok(())
    }
}
