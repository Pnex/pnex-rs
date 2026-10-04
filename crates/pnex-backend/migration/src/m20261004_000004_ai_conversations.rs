//! Assistant conversations (D145, ai-assistant.md §9.4): private to one
//! user within one org, stored relationally so a user can resume, rename,
//! export and erase them (GDPR). Both owners cascade: deleting the user or
//! the org erases their conversations. `busy_until` is the one-turn-at-a-
//! time lease (portable across replicas). Additive: two new tables.

use sea_orm_migration::prelude::*;
use sea_orm_migration::sea_orm::DatabaseBackend;

const POSTGRES_UP: &str = r#"
CREATE TABLE ai_conversations (
    id uuid DEFAULT gen_random_uuid() NOT NULL,
    org_id bigint NOT NULL,
    user_id bigint NOT NULL,
    title character varying(200) NOT NULL,
    busy_until timestamp with time zone,
    last_message_at timestamp with time zone DEFAULT CURRENT_TIMESTAMP NOT NULL,
    created_at timestamp with time zone DEFAULT CURRENT_TIMESTAMP NOT NULL,
    updated_at timestamp with time zone DEFAULT CURRENT_TIMESTAMP NOT NULL,
    CONSTRAINT ai_conversations_pkey PRIMARY KEY (id),
    CONSTRAINT "fk-ai_conversations-org_id" FOREIGN KEY (org_id)
        REFERENCES organizations(id) ON DELETE CASCADE,
    CONSTRAINT "fk-ai_conversations-user_id" FOREIGN KEY (user_id)
        REFERENCES users(id) ON DELETE CASCADE
);
CREATE INDEX idx_ai_conversations_user_org_last ON ai_conversations USING btree (user_id, org_id, last_message_at DESC);
CREATE INDEX idx_ai_conversations_last_message_at ON ai_conversations USING btree (last_message_at);

CREATE TABLE ai_messages (
    id uuid DEFAULT gen_random_uuid() NOT NULL,
    conversation_id uuid NOT NULL,
    seq integer NOT NULL,
    role character varying(16) NOT NULL,
    content text NOT NULL,
    tool_trace jsonb,
    page character varying(255),
    tokens_in integer,
    tokens_out integer,
    created_at timestamp with time zone DEFAULT CURRENT_TIMESTAMP NOT NULL,
    CONSTRAINT ai_messages_pkey PRIMARY KEY (id),
    CONSTRAINT "fk-ai_messages-conversation_id" FOREIGN KEY (conversation_id)
        REFERENCES ai_conversations(id) ON DELETE CASCADE
);
CREATE UNIQUE INDEX uniq_ai_messages_conversation_seq ON ai_messages USING btree (conversation_id, seq);
"#;

const SQLITE_UP: &str = r#"
CREATE TABLE "ai_conversations" ( "id" uuid_text NOT NULL PRIMARY KEY, "org_id" integer NOT NULL, "user_id" integer NOT NULL, "title" varchar(200) NOT NULL, "busy_until" timestamp_with_timezone_text NULL, "last_message_at" timestamp_with_timezone_text NOT NULL DEFAULT CURRENT_TIMESTAMP, "created_at" timestamp_with_timezone_text NOT NULL DEFAULT CURRENT_TIMESTAMP, "updated_at" timestamp_with_timezone_text NOT NULL DEFAULT CURRENT_TIMESTAMP, FOREIGN KEY ("org_id") REFERENCES "organizations" ("id") ON DELETE CASCADE, FOREIGN KEY ("user_id") REFERENCES "users" ("id") ON DELETE CASCADE );
CREATE INDEX idx_ai_conversations_user_org_last ON ai_conversations (user_id, org_id, last_message_at DESC);
CREATE INDEX idx_ai_conversations_last_message_at ON ai_conversations (last_message_at);
CREATE TABLE "ai_messages" ( "id" uuid_text NOT NULL PRIMARY KEY, "conversation_id" uuid_text NOT NULL, "seq" integer NOT NULL, "role" varchar(16) NOT NULL, "content" text NOT NULL, "tool_trace" jsonb_text NULL, "page" varchar(255) NULL, "tokens_in" integer NULL, "tokens_out" integer NULL, "created_at" timestamp_with_timezone_text NOT NULL DEFAULT CURRENT_TIMESTAMP, FOREIGN KEY ("conversation_id") REFERENCES "ai_conversations" ("id") ON DELETE CASCADE );
CREATE UNIQUE INDEX uniq_ai_messages_conversation_seq ON ai_messages (conversation_id, seq);
"#;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, m: &SchemaManager) -> Result<(), DbErr> {
        let script = match m.get_database_backend() {
            DatabaseBackend::Postgres => POSTGRES_UP,
            DatabaseBackend::Sqlite => SQLITE_UP,
            other => {
                return Err(DbErr::Migration(format!(
                    "unsupported database backend: {other:?}"
                )))
            }
        };
        m.get_connection().execute_unprepared(script).await?;
        Ok(())
    }

    async fn down(&self, m: &SchemaManager) -> Result<(), DbErr> {
        m.get_connection()
            .execute_unprepared(
                "DROP TABLE IF EXISTS ai_messages; DROP TABLE IF EXISTS ai_conversations;",
            )
            .await?;
        Ok(())
    }
}
