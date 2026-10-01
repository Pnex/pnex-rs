//! PNeX edge agent library (D95) — the `pnex-agent` binary is a thin CLI
//! over these modules; integration tests drive them directly.

pub mod api;
pub mod config;
pub mod install;
pub mod queue;
pub mod run;
pub mod service;
pub mod state;
pub mod uplink;
