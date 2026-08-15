//! Client API du front — URLs relatives (same-origin), Bearer + X-Org-Id,
//! refresh 401 single-flight, messages d'erreur relayés tels quels.

pub mod auth;
pub mod client;
pub mod config;
pub mod error;
pub mod orgs;
pub mod user;
