//! Mailboxes as sources (mail.md D453–D463): the mailbox (owner or teams,
//! IMAP settings, password in the vault), its cursor per folder, and the
//! messages polled (text only, one per Message-ID and mailbox).

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, m: &SchemaManager) -> Result<(), DbErr> {
        m.get_connection()
            .execute_unprepared(
                "CREATE TABLE mailboxes (
                    id uuid PRIMARY KEY,
                    org_id bigint NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
                    name varchar(120) NOT NULL,
                    address varchar(320) NOT NULL,
                    owner_user_id bigint REFERENCES users(id) ON DELETE CASCADE,
                    teams uuid[] NOT NULL DEFAULT '{}',
                    imap_host varchar(253) NOT NULL,
                    imap_port integer NOT NULL DEFAULT 993,
                    imap_user varchar(320) NOT NULL,
                    secret_id uuid,
                    folders text[] NOT NULL DEFAULT '{INBOX}',
                    state varchar(8) NOT NULL DEFAULT 'active'
                        CHECK (state IN ('active', 'paused', 'error')),
                    last_error_code varchar(64),
                    last_error_args jsonb,
                    last_sync_at timestamptz,
                    retention_days integer NOT NULL DEFAULT 90,
                    version integer NOT NULL DEFAULT 1,
                    created_by bigint REFERENCES users(id) ON DELETE SET NULL,
                    created_at timestamptz NOT NULL DEFAULT now(),
                    updated_at timestamptz NOT NULL DEFAULT now(),
                    UNIQUE (org_id, name)
                );
                CREATE INDEX mailboxes_owner ON mailboxes (owner_user_id);
                CREATE TABLE mail_cursors (
                    mailbox_id uuid NOT NULL REFERENCES mailboxes(id) ON DELETE CASCADE,
                    folder text NOT NULL,
                    uid_validity bigint NOT NULL,
                    last_uid bigint NOT NULL,
                    PRIMARY KEY (mailbox_id, folder)
                );
                CREATE TABLE mail_messages (
                    id uuid PRIMARY KEY,
                    org_id bigint NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
                    mailbox_id uuid NOT NULL REFERENCES mailboxes(id) ON DELETE CASCADE,
                    folder text NOT NULL,
                    uid bigint NOT NULL,
                    message_id varchar(998) NOT NULL,
                    in_reply_to varchar(998),
                    refs text[] NOT NULL DEFAULT '{}',
                    from_name text NOT NULL DEFAULT '',
                    from_address varchar(320) NOT NULL DEFAULT '',
                    to_addrs jsonb NOT NULL DEFAULT '[]'::jsonb,
                    cc_addrs jsonb NOT NULL DEFAULT '[]'::jsonb,
                    subject text NOT NULL DEFAULT '',
                    sent_at timestamptz,
                    body text NOT NULL DEFAULT '',
                    truncated boolean NOT NULL DEFAULT false,
                    attachments jsonb NOT NULL DEFAULT '[]'::jsonb,
                    auth jsonb NOT NULL DEFAULT '{}'::jsonb,
                    automatic boolean NOT NULL DEFAULT false,
                    from_pnex boolean NOT NULL DEFAULT false,
                    forwarded jsonb,
                    received_at timestamptz NOT NULL DEFAULT now(),
                    UNIQUE (mailbox_id, message_id)
                );
                CREATE INDEX mail_messages_box_time ON mail_messages (mailbox_id, received_at DESC);
                CREATE INDEX mail_messages_org_from ON mail_messages (org_id, from_address);",
            )
            .await?;
        Ok(())
    }

    async fn down(&self, m: &SchemaManager) -> Result<(), DbErr> {
        m.get_connection()
            .execute_unprepared(
                "DROP TABLE IF EXISTS mail_messages;
                 DROP TABLE IF EXISTS mail_cursors;
                 DROP TABLE IF EXISTS mailboxes;",
            )
            .await?;
        Ok(())
    }
}
