//! Working state of the watch checks (network-check.md D427): one row per
//! org, check, subject and scope. Holds the fingerprint of the last result,
//! the open event and the run counters, so a run that changes nothing
//! writes no object. Not business data: an object would cost a version per
//! run (exception to K1, recorded in D427).

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, m: &SchemaManager) -> Result<(), DbErr> {
        m.get_connection()
            .execute_unprepared(
                "CREATE TABLE check_states (
                    id bigserial PRIMARY KEY,
                    org_id bigint NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
                    check_key varchar(128) NOT NULL,
                    subject_id uuid NOT NULL,
                    scope_key text NOT NULL,
                    case_id uuid,
                    fingerprint varchar(64) NOT NULL,
                    result varchar(16) NOT NULL,
                    rules jsonb NOT NULL DEFAULT '[]',
                    event_id uuid,
                    source_ref varchar(128) NOT NULL,
                    changed_at timestamptz NOT NULL,
                    first_run_at timestamptz NOT NULL,
                    last_run_at timestamptz NOT NULL,
                    runs bigint NOT NULL DEFAULT 1,
                    runs_since_change bigint NOT NULL DEFAULT 1,
                    UNIQUE (org_id, check_key, subject_id, scope_key)
                );
                CREATE INDEX check_states_case ON check_states (org_id, case_id);",
            )
            .await?;
        Ok(())
    }

    async fn down(&self, m: &SchemaManager) -> Result<(), DbErr> {
        m.get_connection()
            .execute_unprepared("DROP TABLE IF EXISTS check_states;")
            .await?;
        Ok(())
    }
}
