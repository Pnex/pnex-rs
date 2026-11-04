//! Datasets (data-pipelines.md D569): tables stored as Parquet files on S3,
//! versioned by transactions. Committing is a PostgreSQL write, so an open or
//! aborted transaction stays invisible to readers.

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, m: &SchemaManager) -> Result<(), DbErr> {
        m.get_connection()
            .execute_unprepared(
                "CREATE SEQUENCE dataset_commit_seq;
                 CREATE TABLE datasets (
                   id uuid PRIMARY KEY DEFAULT gen_random_uuid(),
                   org_id bigint NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
                   name text NOT NULL,
                   schema jsonb NOT NULL DEFAULT '[]',
                   created_at timestamptz NOT NULL DEFAULT now(),
                   UNIQUE (org_id, name)
                 );
                 CREATE TABLE dataset_transactions (
                   id uuid PRIMARY KEY DEFAULT gen_random_uuid(),
                   org_id bigint NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
                   dataset_id uuid NOT NULL REFERENCES datasets(id) ON DELETE CASCADE,
                   kind text NOT NULL CHECK (kind IN ('snapshot', 'append')),
                   state text NOT NULL DEFAULT 'open'
                     CHECK (state IN ('open', 'committed', 'aborted')),
                   seq bigint,
                   rows bigint NOT NULL DEFAULT 0,
                   bytes bigint NOT NULL DEFAULT 0,
                   code text,
                   heartbeat_at timestamptz NOT NULL DEFAULT now(),
                   created_at timestamptz NOT NULL DEFAULT now(),
                   committed_at timestamptz,
                   purged_at timestamptz
                 );
                 CREATE UNIQUE INDEX dataset_transactions_one_open
                   ON dataset_transactions (dataset_id) WHERE state = 'open';
                 CREATE INDEX dataset_transactions_view
                   ON dataset_transactions (dataset_id, seq) WHERE state = 'committed';
                 CREATE TABLE dataset_files (
                   id uuid PRIMARY KEY DEFAULT gen_random_uuid(),
                   transaction_id uuid NOT NULL
                     REFERENCES dataset_transactions(id) ON DELETE CASCADE,
                   key text NOT NULL,
                   bytes bigint NOT NULL,
                   rows bigint NOT NULL,
                   sha256 text NOT NULL
                 );
                 CREATE INDEX dataset_files_transaction ON dataset_files (transaction_id);",
            )
            .await?;
        Ok(())
    }

    async fn down(&self, m: &SchemaManager) -> Result<(), DbErr> {
        m.get_connection()
            .execute_unprepared(
                "DROP TABLE dataset_files;
                 DROP TABLE dataset_transactions;
                 DROP TABLE datasets;
                 DROP SEQUENCE dataset_commit_seq;",
            )
            .await?;
        Ok(())
    }
}
