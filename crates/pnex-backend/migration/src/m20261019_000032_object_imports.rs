//! Declarative imports into the ontology (partner-watch.md D301, D303): an
//! import holds its mapping, source and schedule; each run is journaled
//! with its counters, and a preview keeps the difference it will apply.

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, m: &SchemaManager) -> Result<(), DbErr> {
        m.get_connection()
            .execute_unprepared(
                "CREATE TABLE object_imports (
                    id uuid PRIMARY KEY,
                    org_id bigint NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
                    name varchar(255) NOT NULL,
                    mapping_yaml text NOT NULL,
                    pack_key varchar(64),
                    source varchar(8) NOT NULL DEFAULT 'file',
                    source_url text,
                    mode varchar(12) NOT NULL DEFAULT 'incremental',
                    recurrence jsonb,
                    next_run_at timestamptz,
                    max_pages integer,
                    version integer NOT NULL DEFAULT 1,
                    created_by bigint REFERENCES users(id) ON DELETE SET NULL,
                    created_at timestamptz NOT NULL DEFAULT now(),
                    updated_at timestamptz NOT NULL DEFAULT now()
                );
                CREATE INDEX idx_object_imports_org ON object_imports (org_id, name);
                CREATE INDEX idx_object_imports_due ON object_imports (next_run_at)
                    WHERE next_run_at IS NOT NULL;
                CREATE TABLE object_import_runs (
                    id uuid PRIMARY KEY,
                    org_id bigint NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
                    import_id uuid NOT NULL REFERENCES object_imports(id) ON DELETE CASCADE,
                    number integer NOT NULL,
                    state varchar(16) NOT NULL,
                    state_code varchar(64),
                    trigger varchar(12) NOT NULL,
                    import_version integer NOT NULL,
                    rows integer NOT NULL DEFAULT 0,
                    counts jsonb NOT NULL DEFAULT '{}',
                    diff jsonb,
                    created_by bigint REFERENCES users(id) ON DELETE SET NULL,
                    started_at timestamptz NOT NULL DEFAULT now(),
                    finished_at timestamptz,
                    CONSTRAINT uniq_object_import_runs_number UNIQUE (import_id, number)
                );
                CREATE INDEX idx_object_import_runs_import
                    ON object_import_runs (import_id, number DESC);",
            )
            .await?;
        Ok(())
    }

    async fn down(&self, m: &SchemaManager) -> Result<(), DbErr> {
        m.get_connection()
            .execute_unprepared(
                "DROP TABLE IF EXISTS object_import_runs; DROP TABLE IF EXISTS object_imports;",
            )
            .await?;
        Ok(())
    }
}
