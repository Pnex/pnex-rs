//! Browser agents (extension-collector.md D254–D269): the credentials of
//! an extension install (kept apart from `device_tokens`, so no device link
//! accepts them) and the missions of the org.

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, m: &SchemaManager) -> Result<(), DbErr> {
        m.get_connection()
            .execute_unprepared(
                "CREATE TABLE extension_agents (
                    device_registry_id bigint PRIMARY KEY REFERENCES device_registries(id) ON DELETE CASCADE,
                    org_id bigint NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
                    install_key text NOT NULL,
                    token_hash varchar(64) NOT NULL,
                    encryption_key text NOT NULL,
                    name varchar(120) NOT NULL,
                    fingerprint jsonb NOT NULL DEFAULT '{}'::jsonb,
                    paired_by bigint,
                    created_at timestamptz NOT NULL DEFAULT now(),
                    updated_at timestamptz NOT NULL DEFAULT now(),
                    CONSTRAINT uniq_extension_agents_install_key UNIQUE (install_key),
                    CONSTRAINT uniq_extension_agents_token_hash UNIQUE (token_hash)
                );
                CREATE INDEX idx_extension_agents_org ON extension_agents (org_id);
                CREATE TABLE extension_missions (
                    id uuid PRIMARY KEY,
                    org_id bigint NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
                    name varchar(120) NOT NULL,
                    kind varchar(32) NOT NULL,
                    spec jsonb NOT NULL,
                    target text NOT NULL,
                    interval_secs integer NOT NULL,
                    agent_id bigint REFERENCES device_registries(id) ON DELETE SET NULL,
                    enabled boolean NOT NULL DEFAULT true,
                    state varchar(16) NOT NULL DEFAULT 'idle',
                    state_code varchar(64),
                    version integer NOT NULL DEFAULT 1,
                    created_by bigint,
                    created_at timestamptz NOT NULL DEFAULT now(),
                    updated_at timestamptz NOT NULL DEFAULT now()
                );
                CREATE INDEX idx_extension_missions_org ON extension_missions (org_id, created_at);",
            )
            .await?;
        Ok(())
    }

    async fn down(&self, m: &SchemaManager) -> Result<(), DbErr> {
        m.get_connection()
            .execute_unprepared(
                "DROP TABLE IF EXISTS extension_missions; DROP TABLE IF EXISTS extension_agents;",
            )
            .await?;
        Ok(())
    }
}
