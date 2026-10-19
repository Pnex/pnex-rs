//! Access management, lot I3 (iam.md D286–D288): custom roles (permission
//! documents of an org), teams of members, and assignments of a role to a
//! member or a team. The managed role of a membership stays the base; an
//! assignment adds to it (and a deny statement takes away).

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, m: &SchemaManager) -> Result<(), DbErr> {
        m.get_connection()
            .execute_unprepared(
                "CREATE TABLE iam_roles (
                    id uuid PRIMARY KEY,
                    org_id bigint NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
                    key varchar(64) NOT NULL,
                    name varchar(120) NOT NULL,
                    description text NOT NULL DEFAULT '',
                    statements jsonb NOT NULL DEFAULT '[]'::jsonb,
                    version integer NOT NULL DEFAULT 1,
                    created_at timestamptz NOT NULL DEFAULT now(),
                    updated_at timestamptz NOT NULL DEFAULT now(),
                    UNIQUE (org_id, key)
                );
                CREATE TABLE iam_teams (
                    id uuid PRIMARY KEY,
                    org_id bigint NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
                    name varchar(120) NOT NULL,
                    description text NOT NULL DEFAULT '',
                    created_at timestamptz NOT NULL DEFAULT now(),
                    UNIQUE (org_id, name)
                );
                CREATE TABLE iam_team_members (
                    team_id uuid NOT NULL REFERENCES iam_teams(id) ON DELETE CASCADE,
                    user_id bigint NOT NULL REFERENCES users(id) ON DELETE CASCADE,
                    PRIMARY KEY (team_id, user_id)
                );
                CREATE INDEX iam_team_members_user ON iam_team_members (user_id);
                CREATE TABLE iam_assignments (
                    id uuid PRIMARY KEY,
                    org_id bigint NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
                    user_id bigint REFERENCES users(id) ON DELETE CASCADE,
                    team_id uuid REFERENCES iam_teams(id) ON DELETE CASCADE,
                    managed_role varchar(16),
                    role_id uuid REFERENCES iam_roles(id) ON DELETE CASCADE,
                    created_by bigint REFERENCES users(id) ON DELETE SET NULL,
                    created_at timestamptz NOT NULL DEFAULT now(),
                    CHECK ((user_id IS NULL) <> (team_id IS NULL)),
                    CHECK ((managed_role IS NULL) <> (role_id IS NULL)),
                    -- Owner stays a membership role (last-owner protection, D117).
                    CHECK (managed_role IS NULL OR managed_role IN ('viewer', 'member', 'admin'))
                );
                CREATE INDEX iam_assignments_org_user ON iam_assignments (org_id, user_id);
                CREATE INDEX iam_assignments_team ON iam_assignments (team_id);",
            )
            .await?;
        Ok(())
    }

    async fn down(&self, m: &SchemaManager) -> Result<(), DbErr> {
        m.get_connection()
            .execute_unprepared(
                "DROP TABLE IF EXISTS iam_assignments;
                 DROP TABLE IF EXISTS iam_team_members;
                 DROP TABLE IF EXISTS iam_teams;
                 DROP TABLE IF EXISTS iam_roles;",
            )
            .await?;
        Ok(())
    }
}
