//! Vision Lab (media-vision-studio.md §4.6, D198–D200): projects of object
//! classes, images of the media library annotated with normalized boxes
//! (jsonb, optimistic `version`), and append-only frozen dataset versions
//! (school `flow_versions`, D18) that exports and trainings read.

use sea_orm_migration::prelude::*;

const UP: &str = r#"
CREATE TABLE lab_projects (
    id          uuid PRIMARY KEY,
    org_id      bigint NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
    name        text NOT NULL,
    description text NOT NULL DEFAULT '',
    classes     jsonb NOT NULL DEFAULT '[]'::jsonb,
    created_by  text NOT NULL DEFAULT '',
    created_at  timestamptz NOT NULL DEFAULT now(),
    updated_at  timestamptz NOT NULL DEFAULT now()
);
CREATE INDEX lab_projects_org ON lab_projects (org_id, updated_at DESC);

CREATE TABLE lab_images (
    id             uuid PRIMARY KEY,
    org_id         bigint NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
    project_id     uuid NOT NULL REFERENCES lab_projects(id) ON DELETE CASCADE,
    media_asset_id uuid NOT NULL REFERENCES media_assets(id) ON DELETE CASCADE,
    name           text NOT NULL DEFAULT '',
    width          integer NOT NULL,
    height         integer NOT NULL,
    source         text NOT NULL,
    split          text NOT NULL DEFAULT 'auto',
    boxes          jsonb NOT NULL DEFAULT '[]'::jsonb,
    negative       boolean NOT NULL DEFAULT false,
    version        integer NOT NULL DEFAULT 1,
    created_at     timestamptz NOT NULL DEFAULT now(),
    updated_at     timestamptz NOT NULL DEFAULT now(),
    UNIQUE (project_id, media_asset_id),
    CONSTRAINT lab_images_split CHECK (split IN ('auto', 'train', 'val'))
);
CREATE INDEX lab_images_project ON lab_images (project_id, created_at);

CREATE TABLE lab_dataset_versions (
    id         uuid PRIMARY KEY,
    org_id     bigint NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
    project_id uuid NOT NULL REFERENCES lab_projects(id) ON DELETE CASCADE,
    version    integer NOT NULL,
    note       text NOT NULL DEFAULT '',
    snapshot   jsonb NOT NULL,
    stats      jsonb NOT NULL,
    created_by text NOT NULL DEFAULT '',
    created_at timestamptz NOT NULL DEFAULT now(),
    UNIQUE (project_id, version)
);
"#;

const DOWN: &str = r#"
DROP TABLE IF EXISTS lab_dataset_versions;
DROP TABLE IF EXISTS lab_images;
DROP TABLE IF EXISTS lab_projects;
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
