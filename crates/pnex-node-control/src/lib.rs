//! Nœuds de flow PNEX — **cartes de régulation mixtes** pour le runtime
//! EdgeLinkd headless :
//!
//! - `pnex-reg-tt-heat` : tout-ou-rien chauffage (hystérésis valeur + anti
//!   court-cycle) ;
//! - `pnex-reg-tt-cool` : tout-ou-rien clim (sens inversé) ;
//! - `pnex-reg-pid` : PID, sortie relais time-proportional.
//!
//! ⚠ Ces nœuds sont **passifs** : la carte mixte décrite par la config est
//! **un seul device** qui lit son propre capteur et pilote sa propre sortie
//! avec le régulateur embarqué côté ESP (D13/D17 — aucune boucle serveur).
//! Le nœud n'est qu'une surface d'authoring : sa config est castée au
//! device par le backend (`ServerMsg::ControlConfig`, cf.
//! docs/architecture/control-cards.md). Le nœud runtime existe pour
//! rétablir le **fail-loud au build** : sans lui, le moteur dégraderait le
//! type inconnu en nœud `unknown` (warn seul, no-op silencieux). Sa boucle
//! `run()` consomme et jette les messages entrants — anti-engorgement si un
//! producteur est câblé en amont.
//!
//! La config est validée au build via la **vraie** fonction
//! `pnex_core::validate_graph` (une seule source de vérité avec le backend
//! et l'éditeur wasm) — un graphe invalide est rejeté au déploiement
//! (`BadFlowsJson` → `redeploy_failed` → exit(1) → respawn).

pub mod regulator;

/// Point d'ancrage référencé par le binaire `pnex-flow-runtime` : garantit
/// que l'édition de liens conserve les soumissions `inventory` de ce crate.
pub fn registered() {}

#[cfg(test)]
mod tests {
    #[test]
    fn registered_ne_panique_pas() {
        super::registered();
    }
}
