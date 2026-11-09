//! The history of object states becomes a pack flag (markets-research.md
//! D914): `history: true` in a pack manifest, copied on its install row,
//! replaces the pack list hard-coded in the capture trigger (000139).

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, m: &SchemaManager) -> Result<(), DbErr> {
        m.get_connection()
            .execute_unprepared(
                "ALTER TABLE packs ADD COLUMN history boolean NOT NULL DEFAULT false;
                 UPDATE packs SET history = true WHERE key IN ('markets', 'world_events');
                 CREATE OR REPLACE FUNCTION object_versions_capture() RETURNS trigger
                 LANGUAGE plpgsql AS $$
                 BEGIN
                   IF (TG_OP = 'INSERT'
                       OR NEW.properties IS DISTINCT FROM OLD.properties
                       OR NEW.title IS DISTINCT FROM OLD.title)
                      AND EXISTS (SELECT 1 FROM object_types t
                                  JOIN packs p ON p.org_id = t.org_id AND p.key = t.pack_key
                                  WHERE t.org_id = NEW.org_id AND t.key = NEW.type_key
                                    AND p.history) THEN
                     INSERT INTO object_versions (object_id, org_id, at, title, properties)
                     VALUES (NEW.id, NEW.org_id, now(), NEW.title, NEW.properties)
                     ON CONFLICT (object_id, at)
                     DO UPDATE SET title = EXCLUDED.title, properties = EXCLUDED.properties;
                   END IF;
                   RETURN NULL;
                 END $$;",
            )
            .await?;
        Ok(())
    }

    async fn down(&self, m: &SchemaManager) -> Result<(), DbErr> {
        m.get_connection()
            .execute_unprepared(
                "CREATE OR REPLACE FUNCTION object_versions_capture() RETURNS trigger
                 LANGUAGE plpgsql AS $$
                 BEGIN
                   IF (TG_OP = 'INSERT'
                       OR NEW.properties IS DISTINCT FROM OLD.properties
                       OR NEW.title IS DISTINCT FROM OLD.title)
                      AND EXISTS (SELECT 1 FROM object_types t
                                  WHERE t.org_id = NEW.org_id AND t.key = NEW.type_key
                                    AND t.pack_key IN ('markets', 'world_events')) THEN
                     INSERT INTO object_versions (object_id, org_id, at, title, properties)
                     VALUES (NEW.id, NEW.org_id, now(), NEW.title, NEW.properties)
                     ON CONFLICT (object_id, at)
                     DO UPDATE SET title = EXCLUDED.title, properties = EXCLUDED.properties;
                   END IF;
                   RETURN NULL;
                 END $$;
                 ALTER TABLE packs DROP COLUMN history;",
            )
            .await?;
        Ok(())
    }
}
