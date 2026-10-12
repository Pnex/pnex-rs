//! Case workspace (cases-workspace.md K30, K33, K44, K45): proposals computed
//! by the detectors (proposed → retained | rejected, never re-proposed after
//! a rejection thanks to the fingerprint) and frozen snapshots of a case
//! (canonical JSON + SHA-256, immutable), and imported tracks (K26): files
//! of past positions kept in Postgres as case evidence, outside the
//! telemetry store and its retention. Everything else of the workspace lives
//! in the ontology (events, attachments, notes).

use sea_orm_migration::prelude::*;

const UP: &str = r#"
CREATE TABLE case_proposals (
    id            uuid PRIMARY KEY,
    org_id        bigint NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
    case_id       uuid NOT NULL REFERENCES objects(id) ON DELETE CASCADE,
    kind          text NOT NULL,
    detector      text NOT NULL,
    params        jsonb NOT NULL DEFAULT '{}'::jsonb,
    fingerprint   text NOT NULL,
    t_start       timestamptz NOT NULL,
    t_end         timestamptz,
    refs          uuid[] NOT NULL DEFAULT '{}',
    payload       jsonb NOT NULL DEFAULT '{}'::jsonb,
    status        text NOT NULL DEFAULT 'proposed',
    reject_reason text,
    event_id      uuid REFERENCES objects(id) ON DELETE SET NULL,
    reviewed_by   bigint REFERENCES users(id) ON DELETE SET NULL,
    reviewed_at   timestamptz,
    created_at    timestamptz NOT NULL DEFAULT now(),
    UNIQUE (case_id, fingerprint),
    CONSTRAINT case_proposals_status CHECK (status IN ('proposed', 'retained', 'rejected')),
    CONSTRAINT case_proposals_span CHECK (t_end IS NULL OR t_end >= t_start)
);
CREATE INDEX case_proposals_case ON case_proposals (case_id, status, t_start);

CREATE TABLE case_snapshots (
    id         uuid PRIMARY KEY,
    org_id     bigint NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
    case_id    uuid NOT NULL REFERENCES objects(id) ON DELETE CASCADE,
    seq        integer NOT NULL,
    label      text,
    taken_by   bigint REFERENCES users(id) ON DELETE SET NULL,
    taken_at   timestamptz NOT NULL DEFAULT now(),
    content    text NOT NULL,
    sha256     char(64) NOT NULL,
    size_bytes integer NOT NULL,
    UNIQUE (case_id, seq)
);

CREATE TABLE track_imports (
    id             uuid PRIMARY KEY,
    org_id         bigint NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
    object_id      uuid NOT NULL REFERENCES objects(id) ON DELETE CASCADE,
    file_name      text NOT NULL,
    format         text NOT NULL,
    sha256         char(64) NOT NULL,
    time_zone      text,
    point_count    integer NOT NULL,
    rejected_count integer NOT NULL,
    t_start        timestamptz NOT NULL,
    t_end          timestamptz NOT NULL,
    imported_by    bigint REFERENCES users(id) ON DELETE SET NULL,
    imported_at    timestamptz NOT NULL DEFAULT now(),
    UNIQUE (object_id, sha256),
    CONSTRAINT track_imports_format CHECK (format IN ('gpx', 'csv'))
);
CREATE INDEX track_imports_object ON track_imports (org_id, object_id);

CREATE TABLE track_points (
    import_id uuid NOT NULL REFERENCES track_imports(id) ON DELETE CASCADE,
    t         timestamptz NOT NULL,
    lon       double precision NOT NULL,
    lat       double precision NOT NULL,
    alt       double precision,
    PRIMARY KEY (import_id, t)
);

CREATE TABLE case_layouts (
    case_id    uuid PRIMARY KEY REFERENCES objects(id) ON DELETE CASCADE,
    org_id     bigint NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
    positions  jsonb NOT NULL,
    updated_by bigint REFERENCES users(id) ON DELETE SET NULL,
    updated_at timestamptz NOT NULL DEFAULT now()
);
"#;

const DOWN: &str = r#"
DROP TABLE IF EXISTS case_layouts;
DROP TABLE IF EXISTS track_points;
DROP TABLE IF EXISTS track_imports;
DROP TABLE IF EXISTS case_snapshots;
DROP TABLE IF EXISTS case_proposals;
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
