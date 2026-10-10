//! Document search P1 (docs/architecture/doc-search.md, decision #22):
//! indexing state of a media version and its lexical text chunks.
//!
//! `text_chunks` is shared with the collaborative pages (pages.md §12):
//! its source is a media version here; the pages migration adds its own
//! nullable source column (real FK) next to `media_version_id`.

use sea_orm_migration::prelude::*;

const UP: &str = r#"
CREATE EXTENSION IF NOT EXISTS pg_trgm;

-- Indexing state of one media version (bytes stay in the MediaStore, D21).
CREATE TABLE media_text_index (
    media_version_id uuid PRIMARY KEY,
    org_id bigint NOT NULL,
    status character varying(16) NOT NULL,
    -- Machine code of err_codes::ALL when status = error.
    error_code character varying(64),
    page_count integer,
    chunk_count integer NOT NULL DEFAULT 0,
    indexed_at timestamp with time zone,
    updated_at timestamp with time zone DEFAULT CURRENT_TIMESTAMP NOT NULL,
    CONSTRAINT "fk-media_text_index-media_version_id" FOREIGN KEY (media_version_id)
        REFERENCES media_versions(id) ON DELETE CASCADE,
    CONSTRAINT "fk-media_text_index-org_id" FOREIGN KEY (org_id)
        REFERENCES organizations(id) ON DELETE CASCADE
);

CREATE TABLE text_chunks (
    id uuid DEFAULT gen_random_uuid() PRIMARY KEY,
    org_id bigint NOT NULL,
    media_version_id uuid,
    ord integer NOT NULL,
    page integer,
    heading text,
    content text NOT NULL,
    -- `simple` keeps codes (E-0457, part refs) intact, `french` stems prose.
    tsv tsvector GENERATED ALWAYS AS (
        setweight(to_tsvector('simple'::regconfig, coalesce(heading, '')), 'A') ||
        to_tsvector('simple'::regconfig, content) ||
        to_tsvector('french'::regconfig, content)
    ) STORED,
    CONSTRAINT "fk-text_chunks-org_id" FOREIGN KEY (org_id)
        REFERENCES organizations(id) ON DELETE CASCADE,
    CONSTRAINT "fk-text_chunks-media_version_id" FOREIGN KEY (media_version_id)
        REFERENCES media_versions(id) ON DELETE CASCADE
);
CREATE INDEX idx_text_chunks_tsv ON text_chunks USING gin (tsv);
-- Near-miss codes (E0457 vs E-0457).
CREATE INDEX idx_text_chunks_trgm ON text_chunks USING gin (content gin_trgm_ops);
CREATE INDEX idx_text_chunks_org ON text_chunks (org_id);
CREATE INDEX idx_text_chunks_media_version ON text_chunks (media_version_id, ord);
"#;

const DOWN: &str = r#"
DROP TABLE IF EXISTS text_chunks;
DROP TABLE IF EXISTS media_text_index;
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
