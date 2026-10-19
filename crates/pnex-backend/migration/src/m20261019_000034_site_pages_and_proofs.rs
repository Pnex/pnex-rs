//! Site page registry and web proofs (partner-watch.md D307–D310): one row
//! per page of a crawled site (`gone` after two crawls without it), and an
//! append-only chain of proofs per org (each manifest names the SHA-256 of
//! the previous one).

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, m: &SchemaManager) -> Result<(), DbErr> {
        m.get_connection()
            .execute_unprepared(
                "CREATE TABLE site_pages (
                    id uuid PRIMARY KEY,
                    org_id bigint NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
                    mission_id uuid REFERENCES extension_missions(id) ON DELETE SET NULL,
                    site text NOT NULL,
                    url text NOT NULL,
                    title text NOT NULL DEFAULT '',
                    depth integer NOT NULL DEFAULT 0,
                    first_seen_at timestamptz NOT NULL DEFAULT now(),
                    last_seen_at timestamptz NOT NULL DEFAULT now(),
                    text_hash varchar(64),
                    captured_hash varchar(64),
                    state varchar(16) NOT NULL DEFAULT 'listed',
                    state_code varchar(64),
                    misses integer NOT NULL DEFAULT 0,
                    doc_asset_id uuid REFERENCES media_assets(id) ON DELETE SET NULL,
                    last_proof_id uuid,
                    object_id uuid,
                    CONSTRAINT uniq_site_pages_url UNIQUE (org_id, url)
                );
                CREATE INDEX idx_site_pages_site ON site_pages (org_id, site, state);
                CREATE TABLE web_proofs (
                    id uuid PRIMARY KEY,
                    org_id bigint NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
                    seq bigint NOT NULL,
                    mission_id uuid REFERENCES extension_missions(id) ON DELETE SET NULL,
                    site_page_id uuid REFERENCES site_pages(id) ON DELETE SET NULL,
                    url text NOT NULL,
                    state varchar(16) NOT NULL DEFAULT 'open',
                    parts jsonb NOT NULL DEFAULT '{}',
                    manifest jsonb,
                    manifest_sha256 varchar(64),
                    prev_sha256 varchar(64),
                    captured_at timestamptz,
                    created_at timestamptz NOT NULL DEFAULT now(),
                    CONSTRAINT uniq_web_proofs_seq UNIQUE (org_id, seq)
                );
                CREATE INDEX idx_web_proofs_page ON web_proofs (site_page_id, created_at DESC);",
            )
            .await?;
        Ok(())
    }

    async fn down(&self, m: &SchemaManager) -> Result<(), DbErr> {
        m.get_connection()
            .execute_unprepared("DROP TABLE IF EXISTS web_proofs; DROP TABLE IF EXISTS site_pages;")
            .await?;
        Ok(())
    }
}
