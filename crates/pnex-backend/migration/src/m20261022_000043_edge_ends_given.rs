//! Bulk link writes (data-pipelines.md D347) pass the identities of both
//! ends: the trigger only resolves an end left empty. Resolving 60,000
//! fresh links one lookup at a time scanned `objects` per row (no
//! statistics yet for the rows the same transaction just inserted).

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, m: &SchemaManager) -> Result<(), DbErr> {
        m.get_connection()
            .execute_unprepared(
                r#"CREATE OR REPLACE FUNCTION pnex_edge_objects() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    IF NEW.source_object_id IS NULL THEN
        SELECT id INTO NEW.source_object_id FROM objects
        WHERE org_id = NEW.org_id AND type_key = NEW.source_kind AND native_id = NEW.source_id;
    END IF;
    IF NEW.target_object_id IS NULL THEN
        SELECT id INTO NEW.target_object_id FROM objects
        WHERE org_id = NEW.org_id AND type_key = NEW.target_kind AND native_id = NEW.target_id;
    END IF;
    RETURN NEW;
END $$;"#,
            )
            .await?;
        Ok(())
    }

    async fn down(&self, m: &SchemaManager) -> Result<(), DbErr> {
        m.get_connection()
            .execute_unprepared(
                r#"CREATE OR REPLACE FUNCTION pnex_edge_objects() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    SELECT id INTO NEW.source_object_id FROM objects
    WHERE org_id = NEW.org_id AND type_key = NEW.source_kind AND native_id = NEW.source_id;
    SELECT id INTO NEW.target_object_id FROM objects
    WHERE org_id = NEW.org_id AND type_key = NEW.target_kind AND native_id = NEW.target_id;
    RETURN NEW;
END $$;"#,
            )
            .await?;
        Ok(())
    }
}
