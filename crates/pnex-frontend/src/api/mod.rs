//! Client API du front — URLs relatives (same-origin), Bearer + X-Org-Id,
//! refresh 401 single-flight, messages d'erreur relayés tels quels.

pub mod auth;
pub mod builds;
pub mod client;
pub mod config;
pub mod dashboard;
pub mod devices;
pub mod error;
pub mod orgs;
pub mod pins;
pub mod telemetry;
pub mod user;
