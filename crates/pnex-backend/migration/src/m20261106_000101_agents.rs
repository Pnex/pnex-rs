//! Agents of an org (agent-environments.md D757, D761): a service account each, with a
//! versioned record of what it runs; runs keep the agent and the version they ran, and the
//! tiers get a cap of agents apart from the human seats.

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, m: &SchemaManager) -> Result<(), DbErr> {
        m.get_connection()
            .execute_unprepared(
                "CREATE TABLE agents (
                   id uuid PRIMARY KEY,
                   org_id bigint NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
                   service_account_id uuid NOT NULL UNIQUE
                     REFERENCES iam_service_accounts(id) ON DELETE CASCADE,
                   name text NOT NULL,
                   description text NOT NULL DEFAULT '',
                   role text NOT NULL CHECK (role IN ('viewer', 'member')),
                   version integer NOT NULL,
                   spec jsonb NOT NULL,
                   created_by bigint REFERENCES users(id) ON DELETE SET NULL,
                   created_at timestamptz NOT NULL DEFAULT now(),
                   updated_at timestamptz NOT NULL DEFAULT now()
                 );
                 CREATE UNIQUE INDEX agents_name ON agents (org_id, lower(name));
                 CREATE TABLE agent_versions (
                   agent_id uuid NOT NULL REFERENCES agents(id) ON DELETE CASCADE,
                   number integer NOT NULL,
                   spec jsonb NOT NULL,
                   created_by bigint REFERENCES users(id) ON DELETE SET NULL,
                   created_at timestamptz NOT NULL DEFAULT now(),
                   PRIMARY KEY (agent_id, number)
                 );
                 ALTER TABLE agent_runs ADD COLUMN agent_id uuid REFERENCES agents(id) ON DELETE SET NULL;
                 ALTER TABLE agent_runs ADD COLUMN agent_version integer;
                 ALTER TABLE agent_runs ADD COLUMN provider_id uuid;
                 CREATE INDEX agent_runs_of_agent ON agent_runs (agent_id, created_at);
                 ALTER TABLE subscription_tiers ADD COLUMN max_agents integer;",
            )
            .await?;
        Ok(())
    }

    async fn down(&self, m: &SchemaManager) -> Result<(), DbErr> {
        m.get_connection()
            .execute_unprepared(
                "ALTER TABLE subscription_tiers DROP COLUMN max_agents;
                 ALTER TABLE agent_runs DROP COLUMN agent_id, DROP COLUMN agent_version,
                   DROP COLUMN provider_id;
                 DROP TABLE agent_versions;
                 DROP TABLE agents;",
            )
            .await?;
        Ok(())
    }
}
