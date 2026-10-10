//! Device placements (D43) expressed as `placed_at` links (ontology.md
//! D191): `device_placements` stays the materialized index the POI pages
//! read, and triggers mirror every change into a temporal link — moving a
//! device closes its link and opens another, removing it closes it. The
//! history of where a device was installed is kept.

use sea_orm_migration::prelude::*;

const UP: &str = r#"
CREATE FUNCTION pnex_placed_at_sync() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    IF TG_OP IN ('UPDATE', 'DELETE') THEN
        UPDATE resource_edges SET valid_to = now(), updated_at = now()
        WHERE org_id = OLD.org_id AND relation = 'placed_at' AND source_kind = 'device'
          AND source_id = OLD.device_registry_id::text AND valid_to IS NULL;
    END IF;
    IF TG_OP IN ('INSERT', 'UPDATE') THEN
        INSERT INTO resource_edges (org_id, relation, source_kind, source_id, target_kind, target_id, source_ref)
        VALUES (NEW.org_id, 'placed_at', 'device', NEW.device_registry_id::text, 'map_pin', NEW.pin_id::text,
                'device_placements');
    END IF;
    RETURN NULL;
END $$;

-- Existing placements become open links, valid since they were made.
INSERT INTO resource_edges (org_id, relation, source_kind, source_id, target_kind, target_id,
                            source_ref, valid_from, created_at)
SELECT org_id, 'placed_at', 'device', device_registry_id::text, 'map_pin', pin_id::text,
       'device_placements', created_at, created_at
FROM device_placements;

CREATE TRIGGER trg_device_placements_placed_at
    AFTER INSERT OR DELETE OR UPDATE OF pin_id ON device_placements
    FOR EACH ROW EXECUTE FUNCTION pnex_placed_at_sync();
"#;

const DOWN: &str = r#"
DROP TRIGGER IF EXISTS trg_device_placements_placed_at ON device_placements;
DROP FUNCTION IF EXISTS pnex_placed_at_sync();
DELETE FROM resource_edges WHERE relation = 'placed_at';
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
