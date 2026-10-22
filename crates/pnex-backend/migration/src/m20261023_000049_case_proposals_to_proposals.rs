//! proposals.md P3: the detections of investigation cases join the shared
//! proposals table (kind `case_event`); `case_proposals` is dropped. The
//! fingerprint is the server's hash of `case_event|<case>|<key>`, `key`
//! being the detection's own fingerprint, so a recomputation still skips
//! what was already proposed, retained or rejected.

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, m: &SchemaManager) -> Result<(), DbErr> {
        m.get_connection()
            .execute_unprepared(
                "ALTER TABLE proposals DROP CONSTRAINT proposals_kind;
                 ALTER TABLE proposals ADD CONSTRAINT proposals_kind CHECK (kind IN
                    ('object_create', 'object_update', 'link_create', 'link_update', 'link_close',
                     'case_event'));
                 INSERT INTO proposals (id, org_id, kind, type_key, target_id, title, vals, refs,
                    origin, slot, fingerprint, status, reason, decided_by, decided_at, applied,
                    created_at)
                 SELECT c.id, c.org_id, 'case_event', 'event', c.case_id::text, c.kind,
                    jsonb_build_object(
                        'key', c.fingerprint, 'detector', c.detector, 'params', c.params,
                        'members', to_jsonb(c.refs::text[]),
                        'from_ms', (extract(epoch FROM c.t_start) * 1000)::bigint,
                        'to_ms', (extract(epoch FROM c.t_end) * 1000)::bigint,
                        'payload', c.payload),
                    array_prepend(c.case_id, c.refs),
                    left('detector:' || c.detector, 255),
                    'case_event|' || c.case_id || '|' || c.fingerprint,
                    encode(sha256(convert_to(
                        'case_event|' || c.case_id || '|' || c.fingerprint, 'UTF8')), 'hex'),
                    CASE c.status WHEN 'retained' THEN 'accepted' ELSE c.status END,
                    c.reject_reason, c.reviewed_by, c.reviewed_at, c.event_id::text, c.created_at
                 FROM case_proposals c
                 ON CONFLICT DO NOTHING;
                 DROP TABLE case_proposals;",
            )
            .await?;
        Ok(())
    }

    async fn down(&self, _m: &SchemaManager) -> Result<(), DbErr> {
        // Data migration: the detections stay in the shared table.
        Ok(())
    }
}
