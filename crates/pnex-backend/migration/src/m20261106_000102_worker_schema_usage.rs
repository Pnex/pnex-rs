//! The pipeline worker reads its tables through the public schema: a managed
//! cluster gives PUBLIC no USAGE on it, so the worker role needs its own.

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, m: &SchemaManager) -> Result<(), DbErr> {
        m.get_connection()
            .execute_unprepared(
                "DO $$ BEGIN
                   IF EXISTS (SELECT 1 FROM pg_roles WHERE rolname = 'pnex_pipeline_worker') THEN
                     GRANT USAGE ON SCHEMA public TO pnex_pipeline_worker;
                   END IF;
                 END $$;",
            )
            .await?;
        Ok(())
    }

    async fn down(&self, m: &SchemaManager) -> Result<(), DbErr> {
        m.get_connection()
            .execute_unprepared(
                "DO $$ BEGIN
                   IF EXISTS (SELECT 1 FROM pg_roles WHERE rolname = 'pnex_pipeline_worker') THEN
                     REVOKE USAGE ON SCHEMA public FROM pnex_pipeline_worker;
                   END IF;
                 END $$;",
            )
            .await?;
        Ok(())
    }
}
