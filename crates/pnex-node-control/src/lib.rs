//! PNEX flow nodes — **mixed control cards** for the headless EdgeLinkd
//! runtime:
//!
//! - `pnex-reg-tt-heat`: on/off heating (value hysteresis + anti
//!   short-cycle);
//! - `pnex-reg-tt-cool`: on/off cooling (reversed direction);
//! - `pnex-reg-pid`: PID, time-proportional relay output.
//!
//! ⚠ These nodes are **passive**: the mixed card described by the config is
//! **a single device** that reads its own sensor and drives its own output
//! with the regulator embedded on the ESP (D13/D17 — no server loop).
//! The node is only an authoring surface: its config is cast to the
//! device by the backend (`ServerMsg::ControlConfig`, see
//! docs/architecture/control-cards.md). The runtime node exists to
//! restore **fail-loud at build**: without it, the engine would degrade the
//! unknown type into an `unknown` node (warn only, silent no-op). Its
//! `run()` loop consumes and drops incoming messages — anti-clogging if a
//! producer is wired upstream.
//!
//! The config is validated at build via the **real**
//! `pnex_core::validate_graph` function (a single source of truth with the backend
//! and the wasm editor) — an invalid graph is rejected at deployment
//! (`BadFlowsJson` → `redeploy_failed` → exit(1) → respawn).

pub mod regulator;

/// Anchor referenced by the `pnex-flow-runtime` binary: guarantees
/// that linking keeps this crate's `inventory` submissions.
pub fn registered() {}

#[cfg(test)]
mod tests {
    #[test]
    fn registered_ne_panique_pas() {
        super::registered();
    }
}
