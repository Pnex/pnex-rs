//! Journal of the open-data ingestion steps (ingestion.md D317–D322): one
//! row per step a flow node ran, with its manifest (node panel, run log),
//! the staging objects it wrote (quota, cleanup, purge) and the sha256 of a
//! download (`skip_if_unchanged`).

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, m: &SchemaManager) -> Result<(), DbErr> {
        m.get_connection()
            .execute_unprepared(
                "CREATE TABLE ingest_steps (
                    id uuid PRIMARY KEY,
                    org_id bigint NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
                    flow_id bigint NOT NULL,
                    flow_version bigint NOT NULL,
                    node_id varchar(64) NOT NULL,
                    run_id uuid NOT NULL,
                    step varchar(24) NOT NULL,
                    port smallint NOT NULL,
                    code varchar(64),
                    manifest jsonb NOT NULL DEFAULT '{}',
                    sha256 varchar(64),
                    keys jsonb NOT NULL DEFAULT '[]',
                    bytes bigint NOT NULL DEFAULT 0,
                    keep_until timestamptz,
                    purged_at timestamptz,
                    created_at timestamptz NOT NULL DEFAULT now()
                );
                CREATE INDEX idx_ingest_steps_flow
                    ON ingest_steps (org_id, flow_id, created_at DESC);
                CREATE INDEX idx_ingest_steps_run ON ingest_steps (run_id);
                CREATE INDEX idx_ingest_steps_live
                    ON ingest_steps (org_id) WHERE purged_at IS NULL;",
            )
            .await?;
        Ok(())
    }

    async fn down(&self, m: &SchemaManager) -> Result<(), DbErr> {
        m.get_connection()
            .execute_unprepared("DROP TABLE IF EXISTS ingest_steps;")
            .await?;
        Ok(())
    }
}
