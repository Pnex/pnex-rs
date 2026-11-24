//! The base each `write: objects` step of a pipeline keeps (pipelines.md D1050):
//! the plan it last applied, gzipped JSON, which its next run diffs against so
//! that an edit made in the ontology survives until the source changes it.

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, m: &SchemaManager) -> Result<(), DbErr> {
        m.get_connection()
            .execute_unprepared(
                "CREATE TABLE pipeline_object_bases (
                   org_id bigint NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
                   pipeline text NOT NULL,
                   step text NOT NULL,
                   mapping jsonb NOT NULL,
                   base bytea NOT NULL,
                   skipped_keys text[] NOT NULL DEFAULT '{}',
                   updated_at timestamptz NOT NULL DEFAULT now(),
                   PRIMARY KEY (org_id, pipeline, step)
                 );",
            )
            .await?;
        Ok(())
    }

    async fn down(&self, m: &SchemaManager) -> Result<(), DbErr> {
        m.get_connection()
            .execute_unprepared("DROP TABLE IF EXISTS pipeline_object_bases;")
            .await?;
        Ok(())
    }
}
