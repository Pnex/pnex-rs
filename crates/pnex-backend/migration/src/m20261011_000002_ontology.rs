//! Ontology core of 0.2.0 (docs/architecture/ontology.md D176–D191,
//! annex A2): universal object identity, org object and link types,
//! temporal typed links, installed packs.
//!
//! Identity is kept by the database itself: a generic trigger gives every
//! row of a system table an `objects` row in the same transaction (every
//! write path, seeds included, without application code), keeps its title
//! in sync and closes its validity on delete. Existing rows are backfilled:
//! a 0.1 database migrates in place (D191).

use sea_orm_migration::prelude::*;

/// `(table, type key, title column)` of the system types (D177).
pub const SYSTEM_TABLES: [(&str, &str, &str); 7] = [
    ("device_registries", "device", "device_id"),
    ("media_assets", "media_asset", "name"),
    ("dashboards", "dashboard", "name"),
    ("tours", "tour", "name"),
    ("map_pins", "map_pin", "label"),
    ("flows", "flow", "name"),
    ("resource_folders", "folder", "name"),
];

const UP: &str = r#"
CREATE TABLE objects (
    id uuid DEFAULT gen_random_uuid() PRIMARY KEY,
    org_id bigint NOT NULL,
    type_key character varying(64) NOT NULL,
    type_version integer,
    title character varying(255) NOT NULL DEFAULT '',
    -- Key of the native row for system types; the object id itself for org types.
    native_id character varying(64) NOT NULL,
    properties jsonb NOT NULL DEFAULT '{}'::jsonb,
    source_ref character varying(255),
    valid_from timestamp with time zone DEFAULT CURRENT_TIMESTAMP NOT NULL,
    valid_to timestamp with time zone,
    version integer NOT NULL DEFAULT 1,
    created_at timestamp with time zone DEFAULT CURRENT_TIMESTAMP NOT NULL,
    updated_at timestamp with time zone DEFAULT CURRENT_TIMESTAMP NOT NULL,
    CONSTRAINT "fk-objects-org_id" FOREIGN KEY (org_id)
        REFERENCES organizations(id) ON UPDATE CASCADE ON DELETE CASCADE
);
CREATE UNIQUE INDEX uniq_objects_org_type_native ON objects (org_id, type_key, native_id);
CREATE INDEX idx_objects_org_type_title ON objects (org_id, type_key, title);
CREATE INDEX idx_objects_properties ON objects USING gin (properties jsonb_path_ops);

CREATE FUNCTION pnex_object_identity() RETURNS trigger LANGUAGE plpgsql AS $$
DECLARE
    t text := left(coalesce(to_jsonb(NEW) ->> TG_ARGV[1], ''), 255);
BEGIN
    IF TG_OP = 'INSERT' OR NEW.object_id IS NULL THEN
        INSERT INTO objects (org_id, type_key, title, native_id)
        VALUES (NEW.org_id, TG_ARGV[0], t, NEW.id::text)
        ON CONFLICT (org_id, type_key, native_id)
            DO UPDATE SET title = EXCLUDED.title, valid_to = NULL, updated_at = now()
        RETURNING id INTO NEW.object_id;
    ELSIF t IS DISTINCT FROM left(coalesce(to_jsonb(OLD) ->> TG_ARGV[1], ''), 255) THEN
        UPDATE objects SET title = t, updated_at = now() WHERE id = NEW.object_id;
    END IF;
    RETURN NEW;
END $$;

-- The identity outlives the row: links and history keep pointing at it.
CREATE FUNCTION pnex_object_close() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    UPDATE objects SET valid_to = now(), updated_at = now()
    WHERE id = OLD.object_id AND valid_to IS NULL;
    RETURN OLD;
END $$;

CREATE TABLE object_types (
    id uuid DEFAULT gen_random_uuid() PRIMARY KEY,
    org_id bigint NOT NULL,
    key character varying(64) NOT NULL,
    current_version integer NOT NULL DEFAULT 1,
    -- Key of the pack that installed the type, if any (D190).
    pack_key character varying(64),
    created_at timestamp with time zone DEFAULT CURRENT_TIMESTAMP NOT NULL,
    updated_at timestamp with time zone DEFAULT CURRENT_TIMESTAMP NOT NULL,
    CONSTRAINT "fk-object_types-org_id" FOREIGN KEY (org_id)
        REFERENCES organizations(id) ON UPDATE CASCADE ON DELETE CASCADE
);
CREATE UNIQUE INDEX uniq_object_types_org_key ON object_types (org_id, key);

CREATE TABLE object_type_versions (
    id uuid DEFAULT gen_random_uuid() PRIMARY KEY,
    org_id bigint NOT NULL,
    object_type_id uuid NOT NULL,
    version integer NOT NULL,
    definition jsonb NOT NULL,
    created_by bigint,
    created_at timestamp with time zone DEFAULT CURRENT_TIMESTAMP NOT NULL,
    CONSTRAINT "fk-object_type_versions-org_id" FOREIGN KEY (org_id)
        REFERENCES organizations(id) ON UPDATE CASCADE ON DELETE CASCADE,
    CONSTRAINT "fk-object_type_versions-object_type_id" FOREIGN KEY (object_type_id)
        REFERENCES object_types(id) ON DELETE CASCADE
);
CREATE UNIQUE INDEX uniq_object_type_versions_type_version
    ON object_type_versions (object_type_id, version);

CREATE TABLE link_types (
    id uuid DEFAULT gen_random_uuid() PRIMARY KEY,
    org_id bigint NOT NULL,
    key character varying(64) NOT NULL,
    definition jsonb NOT NULL,
    version integer NOT NULL DEFAULT 1,
    pack_key character varying(64),
    created_at timestamp with time zone DEFAULT CURRENT_TIMESTAMP NOT NULL,
    updated_at timestamp with time zone DEFAULT CURRENT_TIMESTAMP NOT NULL,
    CONSTRAINT "fk-link_types-org_id" FOREIGN KEY (org_id)
        REFERENCES organizations(id) ON UPDATE CASCADE ON DELETE CASCADE
);
CREATE UNIQUE INDEX uniq_link_types_org_key ON link_types (org_id, key);

CREATE TABLE packs (
    id uuid DEFAULT gen_random_uuid() PRIMARY KEY,
    org_id bigint NOT NULL,
    key character varying(64) NOT NULL,
    version character varying(32) NOT NULL,
    installed_by bigint,
    created_at timestamp with time zone DEFAULT CURRENT_TIMESTAMP NOT NULL,
    updated_at timestamp with time zone DEFAULT CURRENT_TIMESTAMP NOT NULL,
    CONSTRAINT "fk-packs-org_id" FOREIGN KEY (org_id)
        REFERENCES organizations(id) ON UPDATE CASCADE ON DELETE CASCADE
);
CREATE UNIQUE INDEX uniq_packs_org_key ON packs (org_id, key);

-- Links (D179): resource_edges extended in place.
ALTER TABLE resource_edges
    ADD COLUMN attributes jsonb NOT NULL DEFAULT '{}'::jsonb,
    ADD COLUMN valid_from timestamp with time zone DEFAULT CURRENT_TIMESTAMP NOT NULL,
    ADD COLUMN valid_to timestamp with time zone,
    ADD COLUMN source_ref character varying(255),
    ADD COLUMN source_object_id uuid REFERENCES objects(id),
    ADD COLUMN target_object_id uuid REFERENCES objects(id);
-- Existing links are valid since they were created.
UPDATE resource_edges SET valid_from = created_at;
DROP INDEX uniq_resource_edges_pair;
-- A closed link is history: only open links are unique per pair.
CREATE UNIQUE INDEX uniq_resource_edges_open_pair ON resource_edges
    (org_id, relation, source_kind, source_id, target_kind, target_id) WHERE valid_to IS NULL;
CREATE INDEX idx_resource_edges_source_object ON resource_edges (source_object_id);
CREATE INDEX idx_resource_edges_target_object ON resource_edges (target_object_id);

-- Edge ends resolve to identities on write (system and org types alike).
CREATE FUNCTION pnex_edge_objects() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    SELECT id INTO NEW.source_object_id FROM objects
    WHERE org_id = NEW.org_id AND type_key = NEW.source_kind AND native_id = NEW.source_id;
    SELECT id INTO NEW.target_object_id FROM objects
    WHERE org_id = NEW.org_id AND type_key = NEW.target_kind AND native_id = NEW.target_id;
    RETURN NEW;
END $$;
CREATE TRIGGER trg_resource_edges_objects BEFORE INSERT ON resource_edges
    FOR EACH ROW EXECUTE FUNCTION pnex_edge_objects();
"#;

fn system_table_up(table: &str, key: &str, title: &str) -> String {
    format!(
        r#"
ALTER TABLE {table} ADD COLUMN object_id uuid REFERENCES objects(id);
INSERT INTO objects (org_id, type_key, title, native_id, valid_from, created_at)
    SELECT org_id, '{key}', left(coalesce({title}::text, ''), 255), id::text, created_at, created_at
    FROM {table};
UPDATE {table} t SET object_id = o.id FROM objects o
    WHERE o.org_id = t.org_id AND o.type_key = '{key}' AND o.native_id = t.id::text;
CREATE UNIQUE INDEX uniq_{table}_object_id ON {table} (object_id);
-- `UPDATE OF` the title only: hot-path updates (liveness, state) never fire it.
CREATE TRIGGER trg_{table}_object BEFORE INSERT OR UPDATE OF {title} ON {table}
    FOR EACH ROW EXECUTE FUNCTION pnex_object_identity('{key}', '{title}');
CREATE TRIGGER trg_{table}_object_close AFTER DELETE ON {table}
    FOR EACH ROW EXECUTE FUNCTION pnex_object_close();
"#
    )
}

const BACKFILL_EDGES: &str = r#"
UPDATE resource_edges e SET
    source_object_id = (SELECT id FROM objects o WHERE o.org_id = e.org_id
        AND o.type_key = e.source_kind AND o.native_id = e.source_id),
    target_object_id = (SELECT id FROM objects o WHERE o.org_id = e.org_id
        AND o.type_key = e.target_kind AND o.native_id = e.target_id);
"#;

const DOWN: &str = r#"
DROP TRIGGER IF EXISTS trg_resource_edges_objects ON resource_edges;
DROP FUNCTION IF EXISTS pnex_edge_objects();
DROP INDEX IF EXISTS uniq_resource_edges_open_pair;
DELETE FROM resource_edges WHERE valid_to IS NOT NULL;
CREATE UNIQUE INDEX uniq_resource_edges_pair ON resource_edges
    (org_id, relation, source_kind, source_id, target_kind, target_id);
ALTER TABLE resource_edges
    DROP COLUMN attributes, DROP COLUMN valid_from, DROP COLUMN valid_to,
    DROP COLUMN source_ref, DROP COLUMN source_object_id, DROP COLUMN target_object_id;
DROP TABLE IF EXISTS packs, link_types, object_type_versions, object_types;
"#;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, m: &SchemaManager) -> Result<(), DbErr> {
        let db = m.get_connection();
        db.execute_unprepared(UP).await?;
        for (table, key, title) in SYSTEM_TABLES {
            db.execute_unprepared(&system_table_up(table, key, title))
                .await?;
        }
        db.execute_unprepared(BACKFILL_EDGES).await?;
        Ok(())
    }

    async fn down(&self, m: &SchemaManager) -> Result<(), DbErr> {
        let db = m.get_connection();
        for (table, _, _) in SYSTEM_TABLES {
            db.execute_unprepared(&format!(
                "DROP TRIGGER IF EXISTS trg_{table}_object ON {table};
                 DROP TRIGGER IF EXISTS trg_{table}_object_close ON {table};
                 ALTER TABLE {table} DROP COLUMN IF EXISTS object_id;"
            ))
            .await?;
        }
        db.execute_unprepared(DOWN).await?;
        db.execute_unprepared(
            "DROP TABLE IF EXISTS objects;
             DROP FUNCTION IF EXISTS pnex_object_identity();
             DROP FUNCTION IF EXISTS pnex_object_close();",
        )
        .await?;
        Ok(())
    }
}
