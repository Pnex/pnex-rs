//! Geographic layers, phase A (geo-layers.md L1, L4, L5): the served layer
//! and its features in PostGIS. Import jobs, staging, cache and seeds come
//! with phases B–D.
//!
//! PostGIS is created by the cluster (CNPG `postInitApplicationSQL`) where
//! the app user is not superuser: `IF NOT EXISTS` then needs no privilege.

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, m: &SchemaManager) -> Result<(), DbErr> {
        m.get_connection()
            .execute_unprepared(
                "CREATE EXTENSION IF NOT EXISTS postgis;
                 CREATE TABLE geo_layers (
                   id uuid PRIMARY KEY,
                   org_id bigint NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
                   name varchar(255) NOT NULL,
                   description text,
                   geometry_type varchar(16) NOT NULL
                     CHECK (geometry_type IN ('point', 'line', 'polygon')),
                   generation bigint NOT NULL DEFAULT 0,
                   zoom_min smallint NOT NULL DEFAULT 0,
                   zoom_max smallint NOT NULL DEFAULT 22,
                   style jsonb NOT NULL DEFAULT '[]',
                   extent geometry(Geometry, 4326),
                   feature_count bigint NOT NULL DEFAULT 0,
                   created_at timestamptz NOT NULL DEFAULT now(),
                   updated_at timestamptz NOT NULL DEFAULT now(),
                   UNIQUE (org_id, name)
                 );
                 CREATE TABLE geo_features (
                   id bigserial PRIMARY KEY,
                   org_id bigint NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
                   layer_id uuid NOT NULL REFERENCES geo_layers(id) ON DELETE CASCADE,
                   fid text,
                   geom geometry(Geometry, 4326) NOT NULL,
                   geom_3857 geometry GENERATED ALWAYS AS (ST_Transform(geom, 3857)) STORED,
                   props jsonb NOT NULL DEFAULT '{}'
                 );
                 CREATE INDEX geo_features_geom_3857 ON geo_features USING gist (geom_3857);
                 CREATE INDEX geo_features_org_layer ON geo_features (org_id, layer_id);
                 CREATE UNIQUE INDEX geo_features_fid ON geo_features (layer_id, fid)
                   WHERE fid IS NOT NULL;",
            )
            .await?;
        Ok(())
    }

    async fn down(&self, m: &SchemaManager) -> Result<(), DbErr> {
        // The extension stays: other schemas may use it.
        m.get_connection()
            .execute_unprepared("DROP TABLE geo_features; DROP TABLE geo_layers;")
            .await?;
        Ok(())
    }
}
