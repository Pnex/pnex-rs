//! Change feed of the ontology (external-apps.md D435): one row per object
//! or link written, by statement-level triggers (every write path, bulk
//! imports included), ordered by (transaction, sequence) so that a reader
//! never skips a row committed late. Existing objects and links get one
//! row each: the compacted log is a full copy from cursor 0.

use sea_orm_migration::prelude::*;

const UP: &str = r#"
CREATE TABLE ontology_changes (
    seq bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    org_id bigint NOT NULL,
    txid xid8 NOT NULL DEFAULT pg_current_xact_id(),
    at timestamp with time zone NOT NULL DEFAULT now(),
    kind character varying(8) NOT NULL,
    id character varying(64) NOT NULL,
    type_key character varying(64) NOT NULL,
    action character varying(8) NOT NULL
);
CREATE INDEX idx_ontology_changes_feed ON ontology_changes (org_id, txid, seq);
CREATE INDEX idx_ontology_changes_entity ON ontology_changes (org_id, kind, id);

-- Highest position compacted away with a close or a delete, per org: a
-- reader behind it may have missed one (410, full copy).
CREATE TABLE ontology_change_marks (
    org_id bigint PRIMARY KEY,
    txid xid8 NOT NULL,
    seq bigint NOT NULL
);

CREATE FUNCTION pnex_objects_changed() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    IF TG_OP = 'INSERT' THEN
        INSERT INTO ontology_changes (org_id, kind, id, type_key, action)
        SELECT org_id, 'object', id::text, type_key, 'create' FROM new_rows;
    ELSIF TG_OP = 'UPDATE' THEN
        INSERT INTO ontology_changes (org_id, kind, id, type_key, action)
        SELECT n.org_id, 'object', n.id::text, n.type_key,
               CASE WHEN n.valid_to IS NOT NULL AND o.valid_to IS NULL THEN 'close' ELSE 'update' END
        FROM new_rows n JOIN old_rows o ON o.id = n.id
        WHERE (o.*)::text IS DISTINCT FROM (n.*)::text;
    ELSE
        INSERT INTO ontology_changes (org_id, kind, id, type_key, action)
        SELECT org_id, 'object', id::text, type_key, 'delete' FROM old_rows;
    END IF;
    RETURN NULL;
END $$;

CREATE FUNCTION pnex_edges_changed() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    IF TG_OP = 'INSERT' THEN
        INSERT INTO ontology_changes (org_id, kind, id, type_key, action)
        SELECT org_id, 'link', id::text, relation, 'create' FROM new_rows;
    ELSIF TG_OP = 'UPDATE' THEN
        INSERT INTO ontology_changes (org_id, kind, id, type_key, action)
        SELECT n.org_id, 'link', n.id::text, n.relation,
               CASE WHEN n.valid_to IS NOT NULL AND o.valid_to IS NULL THEN 'close' ELSE 'update' END
        FROM new_rows n JOIN old_rows o ON o.id = n.id
        WHERE (o.*)::text IS DISTINCT FROM (n.*)::text;
    ELSE
        INSERT INTO ontology_changes (org_id, kind, id, type_key, action)
        SELECT org_id, 'link', id::text, relation, 'delete' FROM old_rows;
    END IF;
    RETURN NULL;
END $$;

CREATE TRIGGER trg_objects_changes_ins AFTER INSERT ON objects
    REFERENCING NEW TABLE AS new_rows FOR EACH STATEMENT EXECUTE FUNCTION pnex_objects_changed();
CREATE TRIGGER trg_objects_changes_upd AFTER UPDATE ON objects
    REFERENCING OLD TABLE AS old_rows NEW TABLE AS new_rows
    FOR EACH STATEMENT EXECUTE FUNCTION pnex_objects_changed();
CREATE TRIGGER trg_objects_changes_del AFTER DELETE ON objects
    REFERENCING OLD TABLE AS old_rows FOR EACH STATEMENT EXECUTE FUNCTION pnex_objects_changed();
CREATE TRIGGER trg_edges_changes_ins AFTER INSERT ON resource_edges
    REFERENCING NEW TABLE AS new_rows FOR EACH STATEMENT EXECUTE FUNCTION pnex_edges_changed();
CREATE TRIGGER trg_edges_changes_upd AFTER UPDATE ON resource_edges
    REFERENCING OLD TABLE AS old_rows NEW TABLE AS new_rows
    FOR EACH STATEMENT EXECUTE FUNCTION pnex_edges_changed();
CREATE TRIGGER trg_edges_changes_del AFTER DELETE ON resource_edges
    REFERENCING OLD TABLE AS old_rows FOR EACH STATEMENT EXECUTE FUNCTION pnex_edges_changed();

INSERT INTO ontology_changes (org_id, kind, id, type_key, action, at)
SELECT org_id, 'object', id::text, type_key,
       CASE WHEN valid_to IS NULL THEN 'create' ELSE 'close' END, updated_at
FROM objects ORDER BY updated_at;
INSERT INTO ontology_changes (org_id, kind, id, type_key, action, at)
SELECT org_id, 'link', id::text, relation,
       CASE WHEN valid_to IS NULL THEN 'create' ELSE 'close' END, updated_at
FROM resource_edges ORDER BY updated_at;
"#;

const DOWN: &str = r#"
DROP TRIGGER IF EXISTS trg_objects_changes_ins ON objects;
DROP TRIGGER IF EXISTS trg_objects_changes_upd ON objects;
DROP TRIGGER IF EXISTS trg_objects_changes_del ON objects;
DROP TRIGGER IF EXISTS trg_edges_changes_ins ON resource_edges;
DROP TRIGGER IF EXISTS trg_edges_changes_upd ON resource_edges;
DROP TRIGGER IF EXISTS trg_edges_changes_del ON resource_edges;
DROP FUNCTION IF EXISTS pnex_objects_changed();
DROP FUNCTION IF EXISTS pnex_edges_changed();
DROP TABLE IF EXISTS ontology_change_marks, ontology_changes;
"#;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, m: &SchemaManager) -> Result<(), DbErr> {
        m.get_connection().execute_unprepared(UP).await?;
        Ok(())
    }

    async fn down(&self, m: &SchemaManager) -> Result<(), DbErr> {
        m.get_connection().execute_unprepared(DOWN).await?;
        Ok(())
    }
}
