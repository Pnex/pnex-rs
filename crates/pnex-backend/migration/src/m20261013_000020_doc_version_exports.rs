//! PDFs of page versions exported by `page_pdf` nodes (D209): which node
//! already sent which version, so a scheduled flow never sends it twice.

use sea_orm_migration::prelude::*;

const UP: &str = r#"
CREATE TABLE doc_version_exports (
    id             bigserial PRIMARY KEY,
    org_id         bigint NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
    doc_version_id bigint NOT NULL REFERENCES doc_versions(id) ON DELETE CASCADE,
    node_key       text NOT NULL,
    asset_id       uuid REFERENCES media_assets(id) ON DELETE SET NULL,
    created_at     timestamptz NOT NULL DEFAULT now(),
    UNIQUE (doc_version_id, node_key)
);
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
            .execute_unprepared("DROP TABLE IF EXISTS doc_version_exports;")
            .await?;
        Ok(())
    }
}
