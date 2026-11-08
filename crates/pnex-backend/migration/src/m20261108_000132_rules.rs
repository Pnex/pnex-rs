//! Rules (analysis.md D829–D830): a condition on objects or a series, its author,
//! and what the last pass saw, so that only a change to true notifies.

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, m: &SchemaManager) -> Result<(), DbErr> {
        m.get_connection()
            .execute_unprepared(
                "CREATE TABLE rules (
                   id uuid PRIMARY KEY,
                   org_id bigint NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
                   name text NOT NULL,
                   condition jsonb NOT NULL,
                   channel_id uuid REFERENCES notify_channels(id) ON DELETE SET NULL,
                   enabled boolean NOT NULL DEFAULT true,
                   -- What the last pass saw: matching object ids, or whether the series held.
                   state jsonb,
                   firing boolean NOT NULL DEFAULT false,
                   last_fired_at timestamptz,
                   created_by bigint REFERENCES users(id) ON DELETE SET NULL,
                   version integer NOT NULL DEFAULT 1,
                   created_at timestamptz NOT NULL DEFAULT now(),
                   updated_at timestamptz NOT NULL DEFAULT now(),
                   UNIQUE (org_id, name)
                 );",
            )
            .await?;
        Ok(())
    }

    async fn down(&self, m: &SchemaManager) -> Result<(), DbErr> {
        m.get_connection()
            .execute_unprepared("DROP TABLE rules;")
            .await?;
        Ok(())
    }
}
