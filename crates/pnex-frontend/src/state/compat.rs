//! Porte de compatibilité app ↔ serveur : bloque l'app sur un écran dédié si
//! le serveur ne parle pas le même contrat d'API (`pnex_api_contract`),
//! n'est pas joignable, ou n'est pas un serveur PNEX. L'écran bloquant vit
//! dans `pages/gate.rs` ; `main.rs` branche `GATE` entre `SERVER_READY` et le
//! routeur.

use std::time::Duration;

use dioxus::prelude::*;
use pnex_api_contract::{compatible, ServerInfo, CONTRACT};

use crate::api::meta::{self, MetaError};

/// État de la porte — rendu par `GateScreen`.
#[derive(Clone, Debug, PartialEq)]
pub enum GateState {
    /// Vérification en cours — ou sans objet : premier lancement natif sans
    /// URL serveur résolue (l'écran `ServerUrl` prime, la vérification reste
    /// dans cet état).
    Checking,
    /// Serveur conforme et même contrat → routeur.
    Ok,
    /// Serveur PNEX répondant mais contrat différent (avant ou après le
    /// nôtre).
    Incompatible {
        server: Box<ServerInfo>,
        app_contract: u32,
    },
    /// Pas de réponse, statut inattendu (404 d'un backend trop ancien) ou
    /// identité de service non-PNEX.
    Unreachable { error: MetaError },
}

pub static GATE: GlobalSignal<GateState> = GlobalSignal::new(|| GateState::Checking);

/// Vérifie le serveur courant. No-op tant qu'aucune base n'est résolue
/// (`api_base()` vide au premier lancement natif : l'écran de configuration
/// prime et la vérification n'a pas de cible).
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

/// Repasse en vérification avant un changement de serveur (évite le flash de
/// l'écran bloquant périmé pendant la re-vérification).
pub fn reset() {
    GATE.with_mut(|g| *g = GateState::Checking);
}
