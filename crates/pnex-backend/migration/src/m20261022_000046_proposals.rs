//! Proposals (proposals.md D371): writes a machine is not sure are true,
//! waiting for a human; a rejected fingerprint is never proposed again.

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, m: &SchemaManager) -> Result<(), DbErr> {
        m.get_connection()
            .execute_unprepared(
                "CREATE TABLE proposals (
                    id            uuid PRIMARY KEY,
                    org_id        bigint NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
                    kind          text NOT NULL,
                    type_key      text NOT NULL,
                    target_id     text,
                    source_id     text,
                    link_target_id text,
                    title         text,
                    vals          jsonb NOT NULL DEFAULT '{}'::jsonb,
                    base_version  integer,
                    evidence      jsonb NOT NULL DEFAULT 'null'::jsonb,
                    refs          uuid[] NOT NULL DEFAULT '{}',
                    origin        text NOT NULL,
                    confidence    double precision,
                    slot          text NOT NULL,
                    fingerprint   text NOT NULL,
                    status        text NOT NULL DEFAULT 'proposed',
                    reason        text,
                    correction    jsonb,
                    decided_by    bigint REFERENCES users(id) ON DELETE SET NULL,
                    decided_at    timestamptz,
                    applied       text,
                    created_at    timestamptz NOT NULL DEFAULT now(),
                    UNIQUE (org_id, fingerprint),
                    CONSTRAINT proposals_kind CHECK (kind IN
                        ('object_create', 'object_update', 'link_create', 'link_update', 'link_close')),
                    CONSTRAINT proposals_status CHECK (status IN
                        ('proposed', 'accepted', 'rejected', 'superseded')),
                    CONSTRAINT proposals_confidence CHECK (confidence IS NULL OR confidence BETWEEN 0 AND 100)
                );
                CREATE INDEX proposals_inbox ON proposals (org_id, status, created_at DESC);
                CREATE INDEX proposals_slot ON proposals (org_id, slot) WHERE status = 'proposed';
                CREATE INDEX proposals_refs ON proposals USING gin (refs);",
            )
            .await?;
        Ok(())
    }

    async fn down(&self, m: &SchemaManager) -> Result<(), DbErr> {
        m.get_connection()
            .execute_unprepared("DROP TABLE IF EXISTS proposals;")
            .await?;
        Ok(())
    }
}
