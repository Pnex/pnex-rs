//! Services métier (hors contrôleleurs HTTP) : télémétrie, bail de vie,
//! OpenObserve, dashboard, réglages firmware, moteur de flow ETL (D18).

pub mod ai;
/// Annotations sur médias (D55–D60) — écriture des couches versionnées.
pub mod annotation_layer;
pub mod artifact_store;
/// Per-device build history retention (keeps the newest records).
pub mod build_retention;
pub mod camera;
/// Per-pod concurrency caps (CoolProp, vision, runtime checks, uploads).
pub mod compute_limits;
pub mod controls;
pub mod dashboard;
pub mod dashboards;
/// Cross-pod serialization helpers (advisory locks, unique violations).
pub mod db_lock;
pub mod device_bus;
pub mod device_liveness;
/// Graceful drain of buffered background writers at shutdown.
pub mod drain;
pub mod edge_agent;
pub mod events;
pub mod firmware;
pub mod firmware_projects;
pub mod flow;
pub mod flow_cluster;
pub mod flow_supervisor;
pub mod functions;
/// Live last-value telemetry cache (Valkey) — write side (pnex-device-read
/// acceleration).
pub mod last_cache;
pub mod media;
pub mod media_sniff;
pub mod memory;
/// Notifications (D49–D54) — broker WS, journal, pruner, livraison.
pub mod notify;
pub mod notify_journal;
pub mod openobserve;
pub mod ota;
pub mod pois;
pub mod provisioning;
/// Cross-pod rate limiting of unauthenticated / sensitive routes.
pub mod rate_limit;
pub mod regulator;
/// Couche d'organisation transverse (D42) : labels / containment / edges.
pub mod resources;
pub mod retention;
/// Shared Valkey connection for small cross-pod auth/security state.
pub mod runtime_local;
/// Org secrets vault (D110–D118): keyring + AEAD at rest.
pub mod secrets;
pub mod settings;
pub mod shared_valkey;
pub mod singleton;
pub mod stitch;
pub mod surface_controls;
pub mod system_status;
pub mod telemetry;
pub mod tour;
pub mod video;
pub mod video_annotations;
pub mod vision;
pub mod visualization;
