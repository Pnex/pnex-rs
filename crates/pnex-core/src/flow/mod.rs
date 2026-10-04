//! Modèle typé des flows ETL PNEX (décision D18) — source de vérité unique
//! consommée par le backend Loco (validation + projection), par le runtime
//! EdgeLinkd (contrats aux frontières des nœuds custom) et, plus tard, par
//! l'éditeur Dioxus (palette).
//!
//! Contraintes (mêmes règles que le reste du crate) :
//! - pur serde, **aucune dépendance native** — compile natif et `wasm32` ;
//! - `i64` pour tout identifiant qui retourne en base ;
//! - f64/chaînes RFC 3339 pour le temps (pas de chrono).
//!
//! Garde-fou PRD : **pas de type-check à l'échelle du graphe** — la validation
//! couvre la structure du graphe et les contrats aux frontières des nœuds
//! custom uniquement. Les nœuds builtin EdgeLinkd non modélisés passent par
//! [`FlowNodeKind::Red`] (config opaque).
//!
//! # Module layout
//!
//! The former single-file `flow.rs` is split into focused submodules; every
//! public item is re-exported here, so `pnex_core::flow::*` (and the
//! crate-root glob re-exports) resolve exactly as before:
//!
//! - [`config`] — per-kind node configs + pin/reg extractors
//! - [`graph`] — node kinds, wiring, graph root types
//! - [`node_types`] — Red / runtime node-type allowlists (security boundary)
//! - [`payload`] — payload boundary helpers
//! - [`validate`] — graph validation + per-kind contracts
//! - [`red_flows`] — Node-RED `flows.json` projection
//! - [`dto`] — API DTOs (CRUD, versions, runtime, debug feed)

mod config;
mod dto;
mod fence;
mod graph;
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
pub use node_types::*;
pub use payload::*;
pub use red_flows::*;
pub use validate::*;
