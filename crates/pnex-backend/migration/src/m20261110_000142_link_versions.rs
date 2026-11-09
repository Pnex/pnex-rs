//! Links as they were known (markets-research.md D913): every state of a
//! `resource_edges` row, so a read at `as_of` replays what was recorded
//! then instead of today's validity. A link written today "since 2025" does
//! not exist in a backtest of 2025. Written by row triggers: no write path
//! can forget it. A deleted link keeps its versions, emptied and flagged
//! `erased` (Q-BT2): it is absent at every instant.

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, m: &SchemaManager) -> Result<(), DbErr> {
        m.get_connection()
            .execute_unprepared(
                "CREATE TABLE link_versions (
                   edge_id bigint NOT NULL,
                   org_id bigint NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
                   at timestamptz NOT NULL,
                   valid_from timestamptz NOT NULL,
                   valid_to timestamptz,
                   attributes jsonb NOT NULL DEFAULT '{}'::jsonb,
                   source_ref text,
                   erased boolean NOT NULL DEFAULT false,
                   -- Taken over at the migration: an earlier closing or
                   -- attribute change has no known date (Q-BT1).
                   partial boolean NOT NULL DEFAULT false,
                   PRIMARY KEY (edge_id, at)
                 );
                 CREATE INDEX link_versions_org_at ON link_versions (org_id, at);
                 INSERT INTO link_versions
                   (edge_id, org_id, at, valid_from, valid_to, attributes, source_ref, partial)
                 SELECT id, org_id, created_at, valid_from, valid_to, attributes, source_ref,
                        valid_to IS NOT NULL OR updated_at > created_at + interval '1 second'
                 FROM resource_edges;
                 CREATE FUNCTION link_versions_capture() RETURNS trigger
                 LANGUAGE plpgsql AS $$
                 BEGIN
                   IF TG_OP = 'INSERT'
                      OR NEW.valid_from IS DISTINCT FROM OLD.valid_from
                      OR NEW.valid_to IS DISTINCT FROM OLD.valid_to
                      OR NEW.attributes IS DISTINCT FROM OLD.attributes
                      OR NEW.source_ref IS DISTINCT FROM OLD.source_ref THEN
                     INSERT INTO link_versions
                       (edge_id, org_id, at, valid_from, valid_to, attributes, source_ref)
                     VALUES (NEW.id, NEW.org_id, now(), NEW.valid_from, NEW.valid_to,
                             NEW.attributes, NEW.source_ref)
                     ON CONFLICT (edge_id, at) DO UPDATE SET
                       valid_from = EXCLUDED.valid_from, valid_to = EXCLUDED.valid_to,
                       attributes = EXCLUDED.attributes, source_ref = EXCLUDED.source_ref,
                       partial = false;
                   END IF;
                   RETURN NULL;
                 END $$;
                 CREATE TRIGGER link_versions_capture AFTER INSERT OR UPDATE ON resource_edges
                   FOR EACH ROW EXECUTE FUNCTION link_versions_capture();
                 CREATE FUNCTION link_versions_erase() RETURNS trigger
                 LANGUAGE plpgsql AS $$
                 BEGIN
                   UPDATE link_versions SET erased = true, attributes = '{}'::jsonb, source_ref = NULL
                   WHERE edge_id = OLD.id;
                   RETURN NULL;
                 END $$;
                 CREATE TRIGGER link_versions_erase AFTER DELETE ON resource_edges
                   FOR EACH ROW EXECUTE FUNCTION link_versions_erase();",
            )
            .await?;
        Ok(())
    }

    async fn down(&self, m: &SchemaManager) -> Result<(), DbErr> {
        m.get_connection()
            .execute_unprepared(
                "DROP TRIGGER link_versions_erase ON resource_edges;
                 DROP FUNCTION link_versions_erase();
                 DROP TRIGGER link_versions_capture ON resource_edges;
                 DROP FUNCTION link_versions_capture();
                 DROP TABLE link_versions;",
            )
            .await?;
        Ok(())
    }
}
