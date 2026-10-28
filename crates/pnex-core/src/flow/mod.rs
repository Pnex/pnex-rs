//! Typed model of the PNEX ETL flows (decision D18) — single source of truth
//! consumed by the Loco backend (validation + projection), by the EdgeLinkd
//! runtime (contracts at the custom node boundaries) and, later, by
//! the Dioxus editor (palette).
//!
//! Constraints (same rules as the rest of the crate):
//! - pure serde, **no native dependency** — compiles native and `wasm32`;
//! - `i64` for every identifier that goes back to the database;
//! - f64/RFC 3339 strings for time (no chrono).
//!
//! PRD safeguard: **no graph-wide type-check** — validation
//! covers the graph structure and the contracts at the custom node
//! boundaries only. Every node is a typed kind: raw Node-RED builtins are
//! not reachable from a user flow.
//!
//! # Module layout
//!
//! The former single-file `flow.rs` is split into focused submodules; every
//! public item is re-exported here, so `pnex_core::flow::*` (and the
//! crate-root glob re-exports) resolve exactly as before:
//!
//! - [`config`] — per-kind node configs + pin/reg extractors
//! - [`graph`] — node kinds, wiring, graph root types
//! - [`node_docs`] — assistant documentation of every node kind (D142)
//! - [`node_types`] — runtime node-type allowlist (security boundary)
//! - [`payload`] — payload boundary helpers
//! - [`validate`] — graph validation + per-kind contracts
//! - [`red_flows`] — Node-RED `flows.json` projection
//! - [`dto`] — API DTOs (CRUD, versions, runtime, debug feed)

mod config;
mod dto;
mod fence;
mod graph;
mod node_docs;
mod node_types;
mod payload;
mod red_flows;
#[cfg(test)]
mod tests;
mod validate;

pub use config::*;
pub use dto::*;
pub use fence::*;
pub use graph::*;
pub use node_docs::*;
pub use node_types::*;
pub use payload::*;
pub use red_flows::*;
pub use validate::*;
