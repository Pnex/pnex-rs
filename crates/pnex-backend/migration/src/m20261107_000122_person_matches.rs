//! A person entered by a sales rep, matched with a network profile through
//! the search bar of a default rule (agent-unification.md D788): where each match stands, its
//! ladder of queries and its candidates with their scores.

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, m: &SchemaManager) -> Result<(), DbErr> {
        m.get_connection()
            .execute_unprepared(
                "CREATE TABLE person_matches (
                   id uuid PRIMARY KEY,
                   org_id bigint NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
                   person_id uuid NOT NULL,
                   organization_id uuid,
                   state text NOT NULL CONSTRAINT person_matches_state CHECK (state IN
                     ('searching', 'matched', 'proposed', 'not_found', 'stopped')),
                   step integer NOT NULL DEFAULT 0,
                   queries jsonb NOT NULL DEFAULT '[]',
                   subject jsonb NOT NULL DEFAULT '{}',
                   candidates jsonb NOT NULL DEFAULT '[]',
                   state_code text,
                   created_by bigint REFERENCES users(id) ON DELETE SET NULL,
                   created_at timestamptz NOT NULL DEFAULT now(),
                   updated_at timestamptz NOT NULL DEFAULT now()
                 );
                 CREATE INDEX person_matches_person ON person_matches (org_id, person_id, created_at DESC);
                 CREATE INDEX person_matches_open ON person_matches (state)
                   WHERE state IN ('searching', 'proposed');",
            )
            .await?;
        Ok(())
    }

    async fn down(&self, m: &SchemaManager) -> Result<(), DbErr> {
        m.get_connection()
            .execute_unprepared("DROP TABLE person_matches;")
            .await?;
        Ok(())
    }
}
