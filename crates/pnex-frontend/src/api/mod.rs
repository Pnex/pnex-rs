//! Client API du front — URLs relatives (same-origin), Bearer + X-Org-Id,
//! refresh 401 single-flight, messages d'erreur relayés tels quels.

pub mod agents;
pub mod ai;
/// Annotations sur médias (D55–D60) — couches versionnées + read model.
pub mod annotation_layers;
pub mod auth;
pub mod boards;
pub mod builds;
/// Cameras & recordings (camera-video.md D75–D80).
pub mod cameras;
pub mod client;
pub mod config;
/// Org controls (D125–D127): surfaces write, flows listen.
pub mod controls;
pub mod dashboard;
pub mod dashboards;
pub mod fluid_mixtures;
/// Référentiel des hosts serveur PNeX (wizard device).
pub mod hosts;
pub mod meta;
// Consommé uniquement par le pipeline capture360 (cfg android) — mort sur
// les autres cibles, école capture360/mod.rs (allow(dead_code) documenté).
pub mod devices;
pub mod error;
/// Render-time error localization — code → fluent key (`err-<kebab>`).
pub mod error_i18n;
/// JSON events stored in OpenObserve logs (camera-video.md D84).
pub mod events;
/// Custom firmware IDE (custom-firmware.md D87–D94).
pub mod firmware;
pub mod flows;
/// Registre « Fonctions » (groupe « Automation ») — fonctions versionnées.
pub mod functions;
pub mod media;
pub mod memory;
/// Vision model registry (camera-video.md D81).
pub mod ml_models;
/// Notifications (D49–D54) — canaux/templates/journal/tests.
pub mod notify;
pub mod orgs;
pub mod ota;
pub mod pins;
/// Couche d'organisation transverse (D42).
pub mod resources;
/// Global search typeahead (D69).
pub mod search;
/// Org secrets vault (D110–D117).
pub mod secrets;
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
pub mod stitch_jobs;
/// System (D72): O2 retention/cleanup + platform status.
pub mod system;
pub mod telemetry;
pub mod thermo;
/// TLS trust for native targets: pinned edge CA + discovery client (D70).
pub mod tls;
pub mod tours;
pub mod user;
pub mod viz;
/// Référentiel WiFi (wizard device).
pub mod wifi;
