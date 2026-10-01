//! Pages de l'app. Chaque page est un composant routé (cf. `app::Route`) ;
//! `shell::Shell` est le layout racine (sidebar + garde de session).

/// Page dédiée annotations (D55 + porte D60(3)) — flow canonique des
/// couches, indépendant des tours.
pub mod admin_status;
pub mod annotations;
pub mod auth_callback;
/// Cameras: live view, capture settings and recordings (camera-video.md D80).
pub mod cameras;
pub mod catalog;
pub mod dashboard;
pub mod dashboards;
pub mod devices;
/// Référentiels Edge (WiFi / serveurs PNeX) — CRUD consommé par le wizard
/// device et la file de build firmware.
pub mod edge_refs;
/// JSON events written by flows (camera-video.md D84).
pub mod events;
pub mod firmware;
pub mod flows;
pub mod fluid_mixtures;
pub mod functions;
pub mod gate;
pub mod lan_scan;
pub mod login;
pub mod map;
pub mod media;
/// Vision model registry (camera-video.md D81).
pub mod models;
pub mod not_found;
pub mod notifications;
pub mod orgs;
pub mod profile;
pub mod secrets;
pub mod server_url;
pub mod share;
pub mod shell;
/// Vitrine du socle CRUD (/_showcase) — fixtures locales, hors nav.
pub mod showcase;
pub mod studio;
pub mod system;
/// Trust-on-first-use dialog for a self-hosted server root CA (D70).
pub mod trust_ca;
pub mod visualisation;

pub use admin_status::AdminStatus;
pub use annotations::Annotations;
pub use auth_callback::AuthCallback;
pub use cameras::Cameras;
pub use catalog::Catalog;
pub use dashboard::Dashboard;
pub use dashboards::Dashboards;
pub use devices::Devices;
pub use events::Events;
pub use firmware::Firmware;
pub use flows::Flows;
pub use fluid_mixtures::FluidMixtures;
pub use functions::Functions;
pub use map::Map;
pub use media::Media;
pub use models::Models;
pub use not_found::NotFound;
pub use notifications::Notifications;
pub use orgs::{Orgs, OrgsCurrent};
pub use profile::Profile;
pub use secrets::Secrets;
pub use share::ShareTour;
pub use showcase::Showcase;
pub use studio::Studio;
pub use system::System;
pub use visualisation::Visualisation;
