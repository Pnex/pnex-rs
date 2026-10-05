//! Endpoint meta public (`/api/v1/meta/version`) — interrogé HORS du client
//! authentifié partagé (`api::client::send` ajoute Bearer + X-Org-Id et
//! déclenche le refresh 401 single-flight) : l'appel doit réussir avant toute
//! session, et le scan LAN émet des requêtes vers des hôtes inconnus — pas de
//! token vers un inconnu, pas de refresh sur un serveur qui n'est pas le nôtre.

use std::net::SocketAddr;
use std::time::Duration;

use futures::future::{select, Either};
use pnex_api_contract::{ServerInfo, META_VERSION_PATH, SERVICE};

/// Erreurs du endpoint meta — énumérée pour rester traduisible : `t!()` est
/// appelé dans le rsx, jamais dans le helper async (même école que
/// `SetupError` de `pages/server_url.rs`).
#[derive(Clone, Debug, PartialEq)]
pub enum MetaError {
    /// Pas de réponse / statut non-2xx — un backend trop ancien (sans
    /// `/api/v1/meta/version`) répond 404 et atterrit ici.
    Unreachable(String),
    /// Réponse 2xx mais pas un serveur PNEX (JSON inattendu, autre service
    /// sur :5150).
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

/// Interroge `{base}{META_VERSION_PATH}` et valide l'identité du service.
/// `base` est trimée de son slash final (idiome `probe_and_store`) ;
/// `timeout` borne la sonde.
///
/// La borne est une course avec `util::sleep` (futures-timer natif / gloo
/// wasm) et non le seul `RequestBuilder::timeout` : sur Android natif, ce
/// dernier ne s'est PAS déclenché face à un port qui droppe les SYN (constat
/// 2026-09-09 — porte bloquée en « Checking »), tandis que le timer du repo
/// tourne partout (polling login E2E validé sur device).
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

/// Serveur courant (base résolue par le seam `api::config`) — consommé par la
/// porte de compatibilité au boot et par la carte « À propos » du profil.
pub async fn server_info(timeout: Duration) -> Result<ServerInfo, MetaError> {
    probe(&client(), timeout, &crate::api::config::api_base()).await
}

impl MetaError {
    /// Détail réseau relayé tel quel (règle du repo : jamais de traduction
    /// des messages d'erreur) — les timeouts reqwest sont normalisés en
    /// « timeout » pour rester lisibles dans la porte et le scan.
    fn map_reqwest(err: &reqwest::Error) -> Self {
        if err.is_timeout() {
            Self::Unreachable("timeout".to_string())
        } else {
            Self::Unreachable(err.to_string())
        }
    }
}
