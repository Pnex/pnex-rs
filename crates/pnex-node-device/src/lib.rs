//! PNEX **Phase 6** flow nodes (device reads → calculation → OpenObserve
//! metric) for the headless EdgeLinkd runtime — structs registered
//! through `inventory`, typed build, boundary contracts, secrets from env
//! only.
//!
//! - `pnex-device-read`: latest pin values of a single device via
//!   OpenObserve (PromQL `last_over_time`, same series as ingestion) —
//!   one port per pin + an "all" port (combined object);
//! - `pnex-device-write`: writes the output pins of a single device
//!   (digital 1/0, pwm duty 0-100) via the backend internal route;
//! - `pnex-calc`: expression over payload keys (`pnex_core::eval_calc`,
//!   the evaluator shared with the editor);
//! - `pnex-metric`: remote-write of the result — `etl_*` series with virtual
//!   device `flow_{id}`, visible in the Visualization catalogue like a sensor.
//!
//! The OpenObserve org is stamped into the artefact at deploy
//! (`pnex_org_id`), the root creds come from the supervisor's env
//! allowlist — **no secret in flows.json**.

pub mod calc;
pub mod metric;
pub mod o2;
pub mod read;
mod status;
pub mod valkey;
pub mod write;

/// Anchor referenced by the `pnex-flow-runtime` binary: guarantees
/// that linking keeps this crate's `inventory` submissions.
pub fn registered() {}

#[cfg(test)]
mod tests {
    #[test]
    fn registered_does_not_panic() {
        super::registered();
    }
}
