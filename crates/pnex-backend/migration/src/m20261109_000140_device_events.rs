//! Device transitions as dated signals (cases-workspace.md D904, L8b): the
//! reaper's online / offline flips and firmware version changes, written by
//! a trigger on `device_registries` so no write path forgets them. A causal
//! chain reads them as `device.online`, `device.offline`, `change.firmware`.

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, m: &SchemaManager) -> Result<(), DbErr> {
        m.get_connection()
            .execute_unprepared(
                "CREATE TABLE device_events (
                   id bigserial PRIMARY KEY,
                   org_id bigint NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
                   device_registry_id bigint NOT NULL REFERENCES device_registries(id) ON DELETE CASCADE,
                   at timestamptz NOT NULL DEFAULT now(),
                   kind text NOT NULL,
                   detail text
                 );
                 CREATE INDEX device_events_at ON device_events (org_id, device_registry_id, at);
                 CREATE FUNCTION device_events_capture() RETURNS trigger
                 LANGUAGE plpgsql AS $$
                 BEGIN
                   IF NEW.active IS DISTINCT FROM OLD.active THEN
                     INSERT INTO device_events (org_id, device_registry_id, kind)
                     VALUES (NEW.org_id, NEW.id, CASE WHEN NEW.active THEN 'online' ELSE 'offline' END);
                   END IF;
                   IF NEW.fw_version IS DISTINCT FROM OLD.fw_version AND OLD.fw_version IS NOT NULL THEN
                     INSERT INTO device_events (org_id, device_registry_id, kind, detail)
                     VALUES (NEW.org_id, NEW.id, 'firmware', NEW.fw_version);
                   END IF;
                   RETURN NULL;
                 END $$;
                 CREATE TRIGGER device_events_capture AFTER UPDATE ON device_registries
                   FOR EACH ROW EXECUTE FUNCTION device_events_capture();",
            )
            .await?;
        Ok(())
    }

    async fn down(&self, m: &SchemaManager) -> Result<(), DbErr> {
        m.get_connection()
            .execute_unprepared(
                "DROP TRIGGER device_events_capture ON device_registries;
                 DROP FUNCTION device_events_capture();
                 DROP TABLE device_events;",
            )
            .await?;
        Ok(())
    }
}
