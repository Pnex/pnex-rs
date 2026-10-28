//! Front API client — relative URLs (same-origin), Bearer + X-Org-Id,
//! single-flight 401 refresh, error messages relayed as is.

pub mod agents;
pub mod ai;
/// Annotations on media (D55–D60) — versioned layers + read model.
pub mod annotation_layers;
pub mod apps;
pub mod auth;
pub mod boards;
pub mod builds;
/// Cameras & recordings (camera-video.md D75–D80).
pub mod cameras;
pub mod cases;
pub mod client;
pub mod config;
/// Org controls (D125–D127): surfaces write, flows listen.
pub mod controls;
pub mod dashboard;
pub mod dashboards;
pub mod fluid_mixtures;
/// Registry of PNeX server hosts (device wizard).
pub mod hosts;
/// Access management (iam.md §8).
pub mod iam;
pub mod meta;
// Used only by the capture360 pipeline (cfg android) — dead on the
// other targets, capture360/mod.rs pattern (documented allow(dead_code)).
pub mod devices;
// Pages (pages.md #21)
pub mod docs;
pub mod error;
/// Render-time error localization — code → fluent key (`err-<kebab>`).
pub mod error_i18n;
/// JSON events stored in OpenObserve logs (camera-video.md D84).
pub mod events;
/// Browser agents and their missions (extension-collector.md D254–D270).
pub mod extension;
/// Custom firmware IDE (custom-firmware.md D87–D94).
pub mod firmware;
pub mod flows;
/// "Functions" registry ("Automation" group) — versioned functions.
pub mod functions;
pub mod geo;
pub mod lab;
pub mod mailboxes;
pub mod media;
/// Vision model registry (camera-video.md D81).
pub mod media_streams;
pub mod memory;
pub mod ml_models;
/// Notifications (D49–D54) — channels/templates/journal/tests.
pub mod notify;
pub mod ontology;
pub mod orgs;
pub mod ota;
pub mod pins;
pub mod proposals;
/// Cross-cutting organisation layer (D42).
pub mod resources;
/// Global search typeahead (D69).
pub mod search;
/// Org secrets vault (D110–D117).
pub mod secrets;
/// Site navigation rules and their runs (site-recipes.md D323–D326).
pub mod site_recipes;
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
pub mod stitch_jobs;
/// System (D72): O2 retention/cleanup + platform status.
pub mod system;
pub mod taxonomies;
pub mod telemetry;
pub mod thermo;
pub mod time_ranges;
/// TLS trust for native targets: pinned edge CA + discovery client (D70).
pub mod tls;
pub mod tours;
pub mod user;
pub mod viz;
pub mod voice_id;
pub mod web_search;
/// WiFi registry (device wizard).
pub mod wifi;
