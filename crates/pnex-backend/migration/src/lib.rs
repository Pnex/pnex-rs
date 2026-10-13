#![allow(elided_lifetimes_in_paths)]
#![allow(clippy::wildcard_imports)]
//! SeaORM migrations of the PNeX backend (Loco).
//!
//! `m20261009_000001_baseline` is the whole schema of 0.1.0, as plain
//! PostgreSQL (`baseline/postgres.sql`). It replaces the
//! pre-release chain: a database created before it is refused (its applied
//! migrations no longer exist) and must be recreated. Every later change
//! is a new migration following docs/architecture/migrations.md.
//! PostgreSQL is the only supported backend (decision #19).

pub use sea_orm_migration::prelude::*;

mod m20261009_000001_baseline;
pub mod m20261011_000002_ontology;
mod m20261011_000003_doc_search;
mod m20261011_000004_pages;
mod m20261011_000005_geo_providers;
mod m20261011_000010_placed_at_links;
mod m20261011_000011_media_ingest_catchup;
mod m20261011_000012_voice_profiles;
mod m20261012_000013_case_workspace;
mod m20261013_000014_vision_lab;

pub struct Migrator;

#[async_trait::async_trait]
impl MigratorTrait for Migrator {
    fn migrations() -> Vec<Box<dyn MigrationTrait>> {
        vec![
            Box::new(m20261009_000001_baseline::Migration),
            Box::new(m20261011_000002_ontology::Migration),
            Box::new(m20261011_000003_doc_search::Migration),
            Box::new(m20261011_000004_pages::Migration),
            Box::new(m20261011_000005_geo_providers::Migration),
            Box::new(m20261011_000010_placed_at_links::Migration),
            Box::new(m20261011_000011_media_ingest_catchup::Migration),
            Box::new(m20261011_000012_voice_profiles::Migration),
            Box::new(m20261012_000013_case_workspace::Migration),
            Box::new(m20261013_000014_vision_lab::Migration),
        ]
    }
}
