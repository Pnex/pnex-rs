//! The active build of an environment (agent-environments.md D704): chosen by a person,
//! what its runs will execute.

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, m: &SchemaManager) -> Result<(), DbErr> {
        m.get_connection()
            .execute_unprepared(
                "ALTER TABLE environments ADD COLUMN active_build_id uuid
                   REFERENCES environment_builds(id) ON DELETE SET NULL;",
            )
            .await?;
        Ok(())
    }

    async fn down(&self, m: &SchemaManager) -> Result<(), DbErr> {
        m.get_connection()
            .execute_unprepared("ALTER TABLE environments DROP COLUMN active_build_id;")
            .await?;
        Ok(())
    }
}
