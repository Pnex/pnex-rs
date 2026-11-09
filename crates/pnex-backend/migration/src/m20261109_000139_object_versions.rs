//! Object properties as of a date (markets-research.md D900): every state
//! of an object of the markets and world-events packs, kept in PostgreSQL
//! (Q-PT1), so a point-in-time read replays them instead of the current
//! state. Written by a row trigger: no write path can forget it.

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, m: &SchemaManager) -> Result<(), DbErr> {
        m.get_connection()
            .execute_unprepared(
                "CREATE TABLE object_versions (
                   object_id uuid NOT NULL REFERENCES objects(id) ON DELETE CASCADE,
                   org_id bigint NOT NULL,
                   at timestamptz NOT NULL,
                   title text NOT NULL,
                   properties jsonb NOT NULL,
                   PRIMARY KEY (object_id, at)
                 );
                 -- shortcut: the packs with a history are listed here; a
                 -- pack-level flag when a third pack needs one.
                 CREATE FUNCTION object_versions_capture() RETURNS trigger
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
                 CREATE TRIGGER object_versions_capture AFTER INSERT OR UPDATE ON objects
                   FOR EACH ROW EXECUTE FUNCTION object_versions_capture();",
            )
            .await?;
        Ok(())
    }

    async fn down(&self, m: &SchemaManager) -> Result<(), DbErr> {
        m.get_connection()
            .execute_unprepared(
                "DROP TRIGGER object_versions_capture ON objects;
                 DROP FUNCTION object_versions_capture();
                 DROP TABLE object_versions;",
            )
            .await?;
        Ok(())
    }
}
