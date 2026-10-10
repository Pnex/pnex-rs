//! Org geo providers (`geo-layers.md` §8, phase F, L16–L28): basemap,
//! geocoding and routing engines the org plugs in. Configuration, not an
//! ontology object (no `objects` row). The optional API key lives in the
//! vault (`secret_id`, tracked by `secret_usages`, school of
//! `llm_providers`) and is sent as the `key_param` query parameter.
//! Additive: `down()` drops both tables.

use sea_orm_migration::prelude::*;

const UP: &str = r#"
CREATE TABLE geo_providers (
    id uuid DEFAULT gen_random_uuid() NOT NULL PRIMARY KEY,
    org_id bigint NOT NULL,
    name character varying(255) NOT NULL,
    kind character varying(32) NOT NULL,
    capabilities jsonb DEFAULT '[]'::jsonb NOT NULL,
    base_url character varying(2048) NOT NULL,
    secret_id uuid,
    key_param character varying(64) DEFAULT 'key' NOT NULL,
    params jsonb DEFAULT '{}'::jsonb NOT NULL,
    rate_limit_per_s real,
    timeout_ms integer DEFAULT 5000 NOT NULL,
    store_allowed boolean DEFAULT false NOT NULL,
    created_by bigint,
    updated_by bigint,
    created_at timestamp with time zone DEFAULT CURRENT_TIMESTAMP NOT NULL,
    updated_at timestamp with time zone DEFAULT CURRENT_TIMESTAMP NOT NULL,
    CONSTRAINT "fk-geo_providers-org_id" FOREIGN KEY (org_id)
        REFERENCES organizations(id) ON DELETE CASCADE,
    CONSTRAINT "fk-geo_providers-created_by" FOREIGN KEY (created_by)
        REFERENCES users(id) ON DELETE SET NULL,
    CONSTRAINT "fk-geo_providers-updated_by" FOREIGN KEY (updated_by)
        REFERENCES users(id) ON DELETE SET NULL
);
CREATE UNIQUE INDEX uniq_geo_providers_org_name ON geo_providers (org_id, name);

-- One default provider per (org, capability).
CREATE TABLE geo_provider_defaults (
    org_id bigint NOT NULL,
    capability character varying(32) NOT NULL,
    provider_id uuid NOT NULL,
    PRIMARY KEY (org_id, capability),
    CONSTRAINT "fk-geo_provider_defaults-org_id" FOREIGN KEY (org_id)
        REFERENCES organizations(id) ON DELETE CASCADE,
    CONSTRAINT "fk-geo_provider_defaults-provider_id" FOREIGN KEY (provider_id)
        REFERENCES geo_providers(id) ON DELETE CASCADE
);
CREATE INDEX idx_geo_provider_defaults_provider_id ON geo_provider_defaults (provider_id);
"#;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, m: &SchemaManager) -> Result<(), DbErr> {
        m.get_connection().execute_unprepared(UP).await?;
        Ok(())
    }

    async fn down(&self, m: &SchemaManager) -> Result<(), DbErr> {
        m.get_connection()
            .execute_unprepared("DROP TABLE IF EXISTS geo_provider_defaults, geo_providers")
            .await?;
        Ok(())
    }
}
