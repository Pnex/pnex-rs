//! Safe wrapper around the CoolProp C API (v8.0.0), statically linked
//! into the process by [`pnex_coolprop_sys`](pnex_coolprop_sys).
//!
//! All functions are re-exported at the crate root
//! (`use pnex_coolprop::props_si;`). The implementation lives in [`safe`].
//!
//! Origin: adapted from the coolprop-rs project (MIT, same author) — the HTTP
//! server (`coolprop-server`) is replaced here by a direct in-process call.
//!
//! # Usage constraints
//!
//! * CoolProp keeps **process-global** state (config, reference states,
//!   AbstractState handle registry, error string): all
//!   functions are serialised behind a global mutex, and `AbstractState`
//!   handles are lost when the process restarts.
//! * Configuration / reference states are global: do not
//!   mutate them from a multi-tenant server without locking their use.

pub mod plot;
pub mod safe;

pub use plot::*;
pub use safe::*;
