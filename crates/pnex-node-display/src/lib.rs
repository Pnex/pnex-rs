//! PNEX **probe** flow node (`pnex-display`) for the headless EdgeLinkd
//! headless — same convention as [`pnex_node_device`]: registered through
//! `inventory` (`#[flow_node]` macro), typed build, secrets never in
//! flows.json.
//!
//! - **intact** passthrough: the message continues downstream unchanged;
//! - publication to the engine's **debug channel** with the **raw** canvas id
//!   (`pnex_node_id` stamped by the projection — `"n3"`): this is the
//!   attachment key of the editor's live badge and of the debug panel;
//! - captured value = `msg.payload` if present, otherwise the whole message;
//!   the value is published **raw** (the builtin debug pre-stringifies).
//!
//! The identity is stamped by the projection at deploy — never read
//! from a client config (attribution anti-forgery); the backend only exposes
//! anyway what the runtime attributed to the tab (`flow`).

pub mod display;

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
