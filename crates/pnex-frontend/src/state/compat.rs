//! App ↔ server compatibility gate: blocks the app on a dedicated screen if
//! the server does not speak the same API contract (`pnex_api_contract`),
//! is unreachable, or is not a PNEX server. The blocking screen lives
//! in `pages/gate.rs`; `main.rs` wires `GATE` between `SERVER_READY` and the
//! router.

use std::time::Duration;

use dioxus::prelude::*;
use pnex_api_contract::{compatible, ServerInfo, CONTRACT};

use crate::api::meta::{self, MetaError};

/// Gate state — rendered by `GateScreen`.
#[derive(Clone, Debug, PartialEq)]
pub enum GateState {
    /// Check in progress — or not applicable: first native launch without
    /// a resolved server URL (the `ServerUrl` screen prevails, the check stays
    /// in this state).
    Checking,
    /// Compliant server and same contract → router.
    Ok,
    /// PNEX server answering but a different contract (older or newer than
    /// ours).
    Incompatible {
        server: Box<ServerInfo>,
        app_contract: u32,
    },
    /// No answer, unexpected status (404 from a backend too old) or
    /// non-PNEX service identity.
    Unreachable { error: MetaError },
}

pub static GATE: GlobalSignal<GateState> = GlobalSignal::new(|| GateState::Checking);

/// Checks the current server. No-op while no base is resolved
/// (`api_base()` empty at the first native launch: the configuration screen
/// prevails and the check has no target).
pub async fn check() {
    if crate::api::config::api_base().is_empty() {
        return;
    }
    GATE.with_mut(|g| *g = GateState::Checking);
    let state = match meta::server_info(Duration::from_secs(4)).await {
        Ok(info) if compatible(CONTRACT, info.contract) => GateState::Ok,
        Ok(info) => GateState::Incompatible {
            server: Box::new(info),
            app_contract: CONTRACT,
        },
        Err(error) => GateState::Unreachable { error },
    };
    GATE.with_mut(|g| *g = state);
}

/// Goes back to checking before a server change (avoids the flash of the
/// stale blocking screen during the re-check).
pub fn reset() {
    GATE.with_mut(|g| *g = GateState::Checking);
}
