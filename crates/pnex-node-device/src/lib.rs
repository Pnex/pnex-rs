//! Nœuds de flow PNEX **Phase 6** (lecture devices → calcul → métrique
//! OpenObserve) pour le runtime EdgeLinkd headless — structs registered
//! through `inventory`, typed build, boundary contracts, secrets from env
//! only.
//!
//! - `pnex-device-read` : dernières valeurs des pins d'un seul device via
//!   OpenObserve (PromQL `last_over_time`, même série que l'ingestion) —
//!   un port par pin + un port « tout » (objet combiné) ;
//! - `pnex-device-write` : écriture des pins output d'un seul device
//!   (digital 1/0, pwm duty 0-100) via la route interne backend ;
//! - `pnex-calc` : expression sur les clés de payload (`pnex_core::eval_calc`,
//!   l'évaluateur partagé avec l'éditeur) ;
//! - `pnex-metric` : remote-write du résultat — série `etl_*` avec device
//!   virtuel `flow_{id}`, visible au catalogue Visualisation comme un capteur.
//!
//! L'org OpenObserve est estampillée dans l'artefact au deploy
//! (`pnex_org_id`), les creds racine viennent de la allowlist env du
//! superviseur — **aucun secret dans flows.json**.

pub mod calc;
pub mod metric;
pub mod o2;
pub mod read;
pub mod valkey;
pub mod write;

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
