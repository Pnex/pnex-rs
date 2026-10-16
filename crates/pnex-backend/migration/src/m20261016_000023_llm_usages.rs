//! LLM usages (D212): an org routes each usage (assistant, extraction,
//! vision) to one of its providers; no row = its default provider.
//! Deleting the provider drops the row, so the usage falls back silently.

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, m: &SchemaManager) -> Result<(), DbErr> {
        m.get_connection()
            .execute_unprepared(
                "CREATE TABLE llm_usages (
                    org_id bigint NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
                    usage varchar(32) NOT NULL,
                    provider_id uuid NOT NULL REFERENCES llm_providers(id) ON DELETE CASCADE,
                    PRIMARY KEY (org_id, usage)
                );
                CREATE INDEX idx_llm_usages_provider ON llm_usages (provider_id);",
            )
            .await?;
        Ok(())
    }

    async fn down(&self, m: &SchemaManager) -> Result<(), DbErr> {
        m.get_connection()
            .execute_unprepared("DROP TABLE IF EXISTS llm_usages;")
            .await?;
        Ok(())
    }
}
