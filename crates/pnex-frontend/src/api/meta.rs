//! Public meta endpoint (`/api/v1/meta/version`) — queried OUTSIDE the shared
//! authenticated client (`api::client::send` adds Bearer + X-Org-Id and
//! triggers the single-flight 401 refresh): the call must succeed before any
//! session, and the LAN scan sends requests to unknown hosts — no token to a
//! stranger, no refresh on a server that is not ours.

use std::net::SocketAddr;
use std::time::Duration;

use futures::future::{select, Either};
use pnex_api_contract::{ServerInfo, META_VERSION_PATH, SERVICE};

/// Meta endpoint errors — an enum so they stay translatable: `t!()` is
/// called in the rsx, never in the async helper (same approach as
/// `SetupError` in `pages/server_url.rs`).
#[derive(Clone, Debug, PartialEq)]
pub enum MetaError {
    /// No response / non-2xx status — a backend that is too old (without
    /// `/api/v1/meta/version`) answers 404 and lands here.
    Unreachable(String),
    /// 2xx response but not a PNEX server (unexpected JSON, another service
    /// on :5150).
    NotPnex,
}

/// Shared reqwest client for meta calls. No global timeout: reqwest's wasm
/// builder has no `ClientBuilder::timeout` — the timeout is set per request
/// (`RequestBuilder::timeout`, AbortController on wasm) in [`probe`].
/// Trusts the pinned edge CA (native); the LAN scan uses
/// `tls::discovery_client` instead, since it runs before any trust exists.
#[must_use]
pub fn client() -> reqwest::Client {
    crate::api::tls::client(None)
}

/// Queries `{base}{META_VERSION_PATH}` and validates the service identity.
/// `base` is trimmed of its trailing slash (`probe_and_store` idiom);
/// `timeout` bounds the probe.
///
/// The bound is a race with `util::sleep` (native futures-timer / gloo
/// wasm) and not `RequestBuilder::timeout` alone: on native Android, the
/// latter did NOT fire against a port that drops SYNs (observed
/// 2026-09-09 — gate stuck on "Checking"), whereas the repo's timer
/// runs everywhere (E2E login polling validated on device).
pub async fn probe(
    client: &reqwest::Client,
    timeout: Duration,
    base: &str,
) -> Result<ServerInfo, MetaError> {
    probe_with_peer(client, timeout, base)
        .await
        .map(|(info, _)| info)
}

/// [`probe`], plus the socket address the request actually reached (native
/// only; `None` on wasm, where the browser owns the connection). Used to
/// check that an announced origin points at the machine that was probed.
pub async fn probe_with_peer(
    client: &reqwest::Client,
    timeout: Duration,
    base: &str,
) -> Result<(ServerInfo, Option<SocketAddr>), MetaError> {
    let url = format!("{}{META_VERSION_PATH}", base.trim().trim_end_matches('/'));
    let request = Box::pin(async move {
        let response = client
            .get(url)
            .timeout(timeout)
            .send()
            .await
            .map_err(|err| MetaError::map_reqwest(&err))?;
        let status = response.status();
        if !status.is_success() {
            return Err(MetaError::Unreachable(format!("HTTP {status}")));
        }
        let peer = remote_addr(&response);
        let info: ServerInfo = response.json().await.map_err(|_| MetaError::NotPnex)?;
        if info.service != SERVICE {
            return Err(MetaError::NotPnex);
        }
        Ok((info, peer))
    });
    match select(request, Box::pin(crate::util::sleep(timeout))).await {
        Either::Left((result, _)) => result,
        Either::Right(((), _)) => Err(MetaError::Unreachable("timeout".to_string())),
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn remote_addr(response: &reqwest::Response) -> Option<SocketAddr> {
    response.remote_addr()
}

#[cfg(target_arch = "wasm32")]
fn remote_addr(_response: &reqwest::Response) -> Option<SocketAddr> {
    None
}

/// Current server (base resolved by the `api::config` seam) — used by the
/// compatibility gate at boot and by the profile's "About" card.
pub async fn server_info(timeout: Duration) -> Result<ServerInfo, MetaError> {
    probe(&client(), timeout, &crate::api::config::api_base()).await
}

impl MetaError {
    /// Network detail relayed as is (repo rule: error messages are never
    /// translated) — reqwest timeouts are normalised to "timeout" so they
    /// stay readable in the gate and the scan.
    fn map_reqwest(err: &reqwest::Error) -> Self {
        if err.is_timeout() {
            Self::Unreachable("timeout".to_string())
        } else {
            Self::Unreachable(err.to_string())
        }
    }
}
