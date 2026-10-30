//! Sending from a mailbox (mail.md D518–D521): its SMTP server (same user
//! and password as IMAP) and send mode, and the outbox of the mails written
//! from it: drafts a reader sends, and what was sent (the send caps count
//! these rows).

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, m: &SchemaManager) -> Result<(), DbErr> {
        m.get_connection()
            .execute_unprepared(
                r"ALTER TABLE mailboxes
                    ADD COLUMN smtp_host varchar(253) NOT NULL DEFAULT '',
                    ADD COLUMN smtp_port integer NOT NULL DEFAULT 465,
                    ADD COLUMN send_mode varchar(8) NOT NULL DEFAULT 'draft'
                        CHECK (send_mode IN ('draft', 'send'));
                  CREATE TABLE mail_outbox (
                    id uuid PRIMARY KEY,
                    org_id bigint NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
                    mailbox_id uuid NOT NULL REFERENCES mailboxes(id) ON DELETE CASCADE,
                    state varchar(10) NOT NULL DEFAULT 'draft'
                        CHECK (state IN ('draft', 'sending', 'sent', 'failed', 'discarded')),
                    to_addrs text[] NOT NULL,
                    subject text NOT NULL,
                    body text NOT NULL,
                    in_reply_to varchar(998),
                    refs text[] NOT NULL DEFAULT '{}',
                    key varchar(255),
                    origin varchar(255) NOT NULL,
                    automated boolean NOT NULL,
                    message_id varchar(998),
                    error_code varchar(64),
                    error_args jsonb,
                    sent_by bigint REFERENCES users(id) ON DELETE SET NULL,
                    created_at timestamptz NOT NULL DEFAULT now(),
                    sent_at timestamptz,
                    UNIQUE (mailbox_id, key)
                  );
                  CREATE INDEX mail_outbox_box_time ON mail_outbox (mailbox_id, created_at DESC);
                  CREATE INDEX mail_outbox_box_sent ON mail_outbox (mailbox_id, sent_at) WHERE state = 'sent';",
            )
            .await?;
        Ok(())
    }

    async fn down(&self, m: &SchemaManager) -> Result<(), DbErr> {
        m.get_connection()
            .execute_unprepared(
                r"DROP TABLE IF EXISTS mail_outbox;
                  ALTER TABLE mailboxes DROP COLUMN IF EXISTS send_mode,
                    DROP COLUMN IF EXISTS smtp_port, DROP COLUMN IF EXISTS smtp_host;",
            )
            .await?;
        Ok(())
    }
}
