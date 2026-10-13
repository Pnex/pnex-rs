//! Vision Lab training runs (media-vision-studio.md D204): head
//! fine-tuning of a registered YOLOX model on a frozen dataset version, in
//! process, liveness by heartbeat.

use sea_orm_migration::prelude::*;

const UP: &str = r#"
CREATE TABLE lab_training_runs (
    id                 uuid PRIMARY KEY,
    org_id             bigint NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
    project_id         uuid NOT NULL REFERENCES lab_projects(id) ON DELETE CASCADE,
    dataset_version_id uuid NOT NULL REFERENCES lab_dataset_versions(id) ON DELETE CASCADE,
    base_model_id      uuid REFERENCES ml_models(id) ON DELETE SET NULL,
    status             text NOT NULL DEFAULT 'queued',
    epochs             integer NOT NULL,
    progress           jsonb NOT NULL DEFAULT '{}'::jsonb,
    map50              real,
    error              text,
    error_detail       text,
    model_id           uuid REFERENCES ml_models(id) ON DELETE SET NULL,
    created_by         text NOT NULL DEFAULT '',
    created_at         timestamptz NOT NULL DEFAULT now(),
    heartbeat_at       timestamptz NOT NULL DEFAULT now(),
    finished_at        timestamptz,
    CONSTRAINT lab_training_runs_status
        CHECK (status IN ('queued', 'running', 'succeeded', 'failed', 'cancelled'))
);
CREATE INDEX lab_training_runs_project ON lab_training_runs (project_id, created_at DESC);
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
        m.get_connection()
            .execute_unprepared("DROP TABLE IF EXISTS lab_training_runs;")
            .await?;
        Ok(())
    }
}
