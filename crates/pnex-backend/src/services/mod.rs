//! Business services (outside HTTP controllers): telemetry, liveness lease,
//! OpenObserve, dashboard, firmware settings, ETL flow engine (D18).

pub mod agent_artifacts;
pub mod agent_chains;
pub mod agent_gate;
pub mod agent_gateway;
pub mod agent_ops;
pub mod agent_runs;
pub mod agent_skills;
pub mod agents;
pub mod ai;
pub mod analysis;
/// Media annotations (D55–D60) — writes of the versioned layers.
pub mod annotation_layer;
pub mod apps;
pub mod artifact_store;
pub mod build_cache;
/// Per-device build history retention (keeps the newest records).
pub mod build_retention;
/// Large public files into tables: AIS, ADS-B, DVF (markets-research.md MK22–MK24).
pub mod bulk_history;
pub mod camera;
pub mod cmd_acks;
/// Per-pod concurrency caps (CoolProp, vision, runtime checks, uploads).
pub mod compute_limits;
pub mod connectors;
pub mod controls;
pub mod crawl;
pub mod dashboard;
pub mod dashboards;
pub mod data_builds;
pub mod datasets;
/// Cross-pod serialization helpers (advisory locks, unique violations).
pub mod db_lock;
pub mod decide;
pub mod device_bus;
pub mod device_liveness;
pub mod device_pki;
pub mod doc_search;
/// Graceful drain of buffered background writers at shutdown.
pub mod drain;
pub mod edge_agent;
pub mod environments;
pub mod events;
pub mod extension;
pub mod fabric_settings;
pub mod firmware;
pub mod firmware_projects;
pub mod flow;
pub mod flow_ai;
pub mod flow_cluster;
pub mod flow_supervisor;
pub mod forge;
pub mod functions;
pub mod geo;
pub mod governance;
pub mod hooks;
/// Authorization engine and action registry checks (iam.md D285–D299).
pub mod iam;
pub mod image_registries;
pub mod ingest;
pub mod internal_agents;
pub mod job_logs;
/// Live last-value telemetry cache (Valkey) — write side (pnex-device-read
/// acceleration).
pub mod lab;
pub mod lab_train;
pub mod last_cache;
pub mod mail;
pub mod markets;
pub mod mcp;
pub mod media;
pub mod media_analyses;
pub mod media_attributions;
pub mod media_detect;
pub mod media_ingest;
pub mod media_runs;
pub mod media_sniff;
/// Media files sent in parts (data-pipelines.md §9.9).
pub mod media_uploads;
pub mod memory;
/// Notifications (D49–D54) — WS broker, journal, pruner, delivery.
pub mod notify;
pub mod notify_journal;
pub mod notify_recipients;
pub mod notify_templates;
/// Office documents read and written by flows (network-check.md D416, D420).
pub mod office;
pub mod ontology;
pub mod openobserve;
pub mod org_resolve;
pub mod ota;
pub mod ota_signing;
pub mod pages;
pub mod person_match;
pub mod pipelines;
pub mod pois;
pub mod profiles;
pub mod proofs;
pub mod provisioning;
/// Cross-pod rate limiting of unauthenticated / sensitive routes.
pub mod rate_limit;
pub mod regulator;
/// Cross-cutting organization layer (D42): labels / containment / edges.
pub mod resources;
pub mod retention;
pub mod rules;
pub mod runners;
/// Shared Valkey connection for small cross-pod auth/security state.
pub mod runtime_local;
pub mod scan_journal;
/// Org secrets vault (D110–D118): keyring + AEAD at rest.
pub mod secrets;
pub mod settings;
pub mod shared_valkey;
pub mod singleton;
pub mod site_crawl;
/// Site navigation rules and the server browser (site-recipes.md D323–D326).
pub mod site_recipes;
pub mod software_supply;
pub mod stitch;
pub mod storage_reconcile;
pub mod surface_controls;
pub mod system_status;
pub mod telemetry;
/// Time ranges (media-ingest.md D169): CRUD, upsert, CSV/ICS import.
pub mod telemetry_aggregate;
pub mod time_ranges;
pub mod tour;
pub mod video;
pub mod video_annotations;
pub mod vision;
pub mod visualization;
pub mod voice_id;
pub mod web_search;
pub mod web_visit;
pub mod ws_ticket;
