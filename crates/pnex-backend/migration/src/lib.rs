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
mod m20261013_000015_lab_frame_position;
mod m20261013_000016_lab_training_runs;
mod m20261013_000017_lab_image_luma;
mod m20261013_000018_lab_review_queue;
mod m20261013_000019_doc_version_review;
mod m20261013_000020_doc_version_exports;
mod m20261014_000021_drop_firmware_artifacts;
mod m20261015_000022_label_purge;
mod m20261016_000023_llm_usages;
mod m20261016_000024_media_analyses;
mod m20261018_000025_browser_agents;
mod m20261018_000026_extension_mission_cursor;
mod m20261018_000027_social_videos;
mod m20261018_000028_media_stream_pace;
mod m20261018_000029_media_runs;
mod m20261018_000030_speech_attributions;
mod m20261018_000031_extension_agent_mode;
mod m20261019_000032_object_imports;
mod m20261019_000033_notify_dynamic_sends;
mod m20261019_000034_site_pages_and_proofs;
mod m20261019_000035_iam_roles_teams;
mod m20261019_000036_iam_service_accounts;
mod m20261020_000037_ingest_steps;
mod m20261020_000038_site_recipes;
mod m20261020_000039_browser_endpoints;
mod m20261020_000040_site_recipe_versions;
mod m20261021_000041_prospection_oppositions;
mod m20261022_000042_import_run_heartbeat;
mod m20261022_000043_edge_ends_given;
mod m20261022_000044_web_search_providers;
mod m20261022_000045_import_per_object_cursor;

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
            Box::new(m20261013_000015_lab_frame_position::Migration),
            Box::new(m20261013_000016_lab_training_runs::Migration),
            Box::new(m20261013_000017_lab_image_luma::Migration),
            Box::new(m20261013_000018_lab_review_queue::Migration),
            Box::new(m20261013_000019_doc_version_review::Migration),
            Box::new(m20261013_000020_doc_version_exports::Migration),
            Box::new(m20261014_000021_drop_firmware_artifacts::Migration),
            Box::new(m20261015_000022_label_purge::Migration),
            Box::new(m20261016_000023_llm_usages::Migration),
            Box::new(m20261016_000024_media_analyses::Migration),
            Box::new(m20261018_000025_browser_agents::Migration),
            Box::new(m20261018_000026_extension_mission_cursor::Migration),
            Box::new(m20261018_000027_social_videos::Migration),
            Box::new(m20261018_000028_media_stream_pace::Migration),
            Box::new(m20261018_000029_media_runs::Migration),
            Box::new(m20261018_000030_speech_attributions::Migration),
            Box::new(m20261018_000031_extension_agent_mode::Migration),
            Box::new(m20261019_000032_object_imports::Migration),
            Box::new(m20261019_000033_notify_dynamic_sends::Migration),
            Box::new(m20261019_000034_site_pages_and_proofs::Migration),
            Box::new(m20261019_000035_iam_roles_teams::Migration),
            Box::new(m20261019_000036_iam_service_accounts::Migration),
            Box::new(m20261020_000037_ingest_steps::Migration),
            Box::new(m20261020_000038_site_recipes::Migration),
            Box::new(m20261020_000039_browser_endpoints::Migration),
            Box::new(m20261020_000040_site_recipe_versions::Migration),
            Box::new(m20261021_000041_prospection_oppositions::Migration),
            Box::new(m20261022_000042_import_run_heartbeat::Migration),
            Box::new(m20261022_000043_edge_ends_given::Migration),
            Box::new(m20261022_000044_web_search_providers::Migration),
            Box::new(m20261022_000045_import_per_object_cursor::Migration),
        ]
    }
}
