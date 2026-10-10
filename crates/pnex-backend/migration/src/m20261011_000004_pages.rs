//! Pages L1–L2 (`docs/architecture/pages.md`, decision #21): collaborative
//! documents stored as CRDT state.
//!
//! - `docs`: one row per page; `snapshot` is the compacted CRDT state,
//!   `snapshot_seq` the last update folded into it (P4).
//! - `doc_updates`: append-only CRDT update log since the snapshot; the
//!   compaction folds rows `seq <= snapshot_seq` into `docs.snapshot` and
//!   deletes them.
//! - `doc_versions`: named versions, append-only (P5, D18 school), each
//!   with its frozen CRDT state.
//! - `doc_comments`: comment threads anchored on a text range (P12). The
//!   anchor is a pair of Yjs relative positions (opaque to the server), so
//!   it survives edits; `quote` keeps the text for an anchor that is gone.
//! - Universal identity (`ontology.md`): `docs.object_id` is filled by the
//!   generic `pnex_object_identity` trigger of the ontology migration.

use sea_orm_migration::prelude::*;

const UP: &str = r#"
CREATE TABLE docs (
    id uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    org_id bigint NOT NULL,
    title character varying(255) NOT NULL,
    snapshot bytea,
    snapshot_seq bigint DEFAULT 0 NOT NULL,
    created_by bigint,
    object_id uuid REFERENCES objects(id),
    created_at timestamp with time zone DEFAULT CURRENT_TIMESTAMP NOT NULL,
    updated_at timestamp with time zone DEFAULT CURRENT_TIMESTAMP NOT NULL,
    CONSTRAINT "fk-docs-org_id-to-organizations" FOREIGN KEY (org_id)
        REFERENCES organizations(id) ON UPDATE CASCADE ON DELETE CASCADE,
    CONSTRAINT "fk-docs-created_by-to-users" FOREIGN KEY (created_by)
        REFERENCES users(id) ON DELETE SET NULL
);
CREATE INDEX idx_docs_org_updated ON docs USING btree (org_id, updated_at DESC);

CREATE TABLE doc_updates (
    id bigserial PRIMARY KEY,
    doc_id uuid NOT NULL,
    data bytea NOT NULL,
    user_id bigint,
    created_at timestamp with time zone DEFAULT CURRENT_TIMESTAMP NOT NULL,
    CONSTRAINT "fk-doc_updates-doc_id-to-docs" FOREIGN KEY (doc_id)
        REFERENCES docs(id) ON DELETE CASCADE
);
CREATE INDEX idx_doc_updates_doc_id ON doc_updates USING btree (doc_id, id);

CREATE TABLE doc_versions (
    id bigserial PRIMARY KEY,
    doc_id uuid NOT NULL,
    org_id bigint NOT NULL,
    version_number bigint NOT NULL,
    label character varying(255) NOT NULL,
    snapshot bytea NOT NULL,
    created_by bigint,
    created_at timestamp with time zone DEFAULT CURRENT_TIMESTAMP NOT NULL,
    CONSTRAINT "fk-doc_versions-doc_id-to-docs" FOREIGN KEY (doc_id)
        REFERENCES docs(id) ON DELETE CASCADE,
    CONSTRAINT "fk-doc_versions-org_id-to-organizations" FOREIGN KEY (org_id)
        REFERENCES organizations(id) ON UPDATE CASCADE ON DELETE CASCADE,
    CONSTRAINT "fk-doc_versions-created_by-to-users" FOREIGN KEY (created_by)
        REFERENCES users(id) ON DELETE SET NULL
);
CREATE UNIQUE INDEX uniq_doc_versions_doc_number ON doc_versions USING btree (doc_id, version_number);

-- Identity row (`objects`), same shape as the ontology's system_table_up().
CREATE UNIQUE INDEX uniq_docs_object_id ON docs (object_id);
CREATE TABLE doc_comments (
    id bigserial PRIMARY KEY,
    doc_id uuid NOT NULL,
    org_id bigint NOT NULL,
    parent_id bigint,
    anchor text,
    quote text,
    body text NOT NULL,
    author_id bigint,
    resolved boolean DEFAULT false NOT NULL,
    resolved_by bigint,
    version_number bigint,
    created_at timestamp with time zone DEFAULT CURRENT_TIMESTAMP NOT NULL,
    updated_at timestamp with time zone DEFAULT CURRENT_TIMESTAMP NOT NULL,
    CONSTRAINT "fk-doc_comments-doc_id-to-docs" FOREIGN KEY (doc_id)
        REFERENCES docs(id) ON DELETE CASCADE,
    CONSTRAINT "fk-doc_comments-org_id-to-organizations" FOREIGN KEY (org_id)
        REFERENCES organizations(id) ON UPDATE CASCADE ON DELETE CASCADE,
    CONSTRAINT "fk-doc_comments-parent_id-to-doc_comments" FOREIGN KEY (parent_id)
        REFERENCES doc_comments(id) ON DELETE CASCADE,
    CONSTRAINT "fk-doc_comments-author_id-to-users" FOREIGN KEY (author_id)
        REFERENCES users(id) ON DELETE SET NULL,
    CONSTRAINT "fk-doc_comments-resolved_by-to-users" FOREIGN KEY (resolved_by)
        REFERENCES users(id) ON DELETE SET NULL
);
CREATE INDEX idx_doc_comments_doc_id ON doc_comments USING btree (doc_id, id);

CREATE TRIGGER trg_docs_object BEFORE INSERT OR UPDATE OF title ON docs
    FOR EACH ROW EXECUTE FUNCTION pnex_object_identity('doc', 'title');
CREATE TRIGGER trg_docs_object_close AFTER DELETE ON docs
    FOR EACH ROW EXECUTE FUNCTION pnex_object_close();
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
            .execute_unprepared(
                "DROP TABLE IF EXISTS doc_comments, doc_versions, doc_updates, docs CASCADE",
            )
            .await?;
        Ok(())
    }
}
