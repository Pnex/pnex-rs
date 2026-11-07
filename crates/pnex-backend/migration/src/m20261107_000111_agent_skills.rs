//! Skills of hosted agents (agent-environments.md D711, autonomous-agents.md D396): versioned
//! text of an org. A text is a draft until a person activates it; agents pin activated ones.

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, m: &SchemaManager) -> Result<(), DbErr> {
        m.get_connection()
            .execute_unprepared(
                "CREATE TABLE agent_skills (
                   id uuid PRIMARY KEY,
                   org_id bigint NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
                   name text NOT NULL,
                   description text NOT NULL DEFAULT '',
                   version integer NOT NULL DEFAULT 1,
                   latest_number integer NOT NULL DEFAULT 1,
                   active_number integer,
                   created_by bigint REFERENCES users(id) ON DELETE SET NULL,
                   created_at timestamptz NOT NULL DEFAULT now(),
                   updated_at timestamptz NOT NULL DEFAULT now()
                 );
                 CREATE UNIQUE INDEX agent_skills_name ON agent_skills (org_id, name);
                 CREATE TABLE agent_skill_versions (
                   skill_id uuid NOT NULL REFERENCES agent_skills(id) ON DELETE CASCADE,
                   number integer NOT NULL,
                   body text NOT NULL,
                   sha256 text NOT NULL,
                   created_by bigint REFERENCES users(id) ON DELETE SET NULL,
                   created_at timestamptz NOT NULL DEFAULT now(),
                   activated_by bigint REFERENCES users(id) ON DELETE SET NULL,
                   activated_at timestamptz,
                   PRIMARY KEY (skill_id, number)
                 );",
            )
            .await?;
        Ok(())
    }

    async fn down(&self, m: &SchemaManager) -> Result<(), DbErr> {
        m.get_connection()
            .execute_unprepared(
                "DROP TABLE agent_skill_versions;
                 DROP TABLE agent_skills;",
            )
            .await?;
        Ok(())
    }
}
