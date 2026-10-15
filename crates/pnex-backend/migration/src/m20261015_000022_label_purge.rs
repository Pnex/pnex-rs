//! Labels on label-only kinds (D218): functions, firmware projects,
//! controls, streams, ML models, notification channels and templates,
//! annotation layers carry labels without being ontology types. Deleting
//! one of their rows drops its labels in the database, so no delete
//! handler can forget it.

use sea_orm_migration::prelude::*;

/// (table, label kind) — the kinds of `pnex_core::resources::LABEL_ONLY_KINDS`.
const TABLES: [(&str, &str); 8] = [
    ("functions", "function"),
    ("firmware_projects", "firmware_project"),
    ("controls", "control"),
    ("media_streams", "media_stream"),
    ("ml_models", "ml_model"),
    ("notify_channels", "notify_channel"),
    ("notify_templates", "notify_template"),
    ("annotation_layers", "annotation_layer"),
];

const FUNCTION: &str = r#"
CREATE FUNCTION pnex_labels_purge() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    DELETE FROM resource_labels
    WHERE org_id = OLD.org_id AND resource_kind = TG_ARGV[0] AND resource_id = OLD.id::text;
    RETURN OLD;
END $$;
"#;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, m: &SchemaManager) -> Result<(), DbErr> {
        let db = m.get_connection();
        db.execute_unprepared(FUNCTION).await?;
        for (table, kind) in TABLES {
            db.execute_unprepared(&format!(
                "CREATE TRIGGER trg_{table}_labels_purge AFTER DELETE ON {table} \
                 FOR EACH ROW EXECUTE FUNCTION pnex_labels_purge('{kind}');"
            ))
            .await?;
        }
        Ok(())
    }

    async fn down(&self, m: &SchemaManager) -> Result<(), DbErr> {
        let db = m.get_connection();
        for (table, _) in TABLES {
            db.execute_unprepared(&format!(
                "DROP TRIGGER IF EXISTS trg_{table}_labels_purge ON {table};"
            ))
            .await?;
        }
        db.execute_unprepared("DROP FUNCTION IF EXISTS pnex_labels_purge();")
            .await?;
        Ok(())
    }
}
