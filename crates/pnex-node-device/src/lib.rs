//! Nœuds de flow PNEX **Phase 6** (lecture devices → calcul → métrique
//! OpenObserve) pour le runtime EdgeLinkd headless — même convention que
//! [`pnex_node_sql`] : trois structs enregistrés par `inventory`, build
//! typé, contrats aux frontières, secrets par env uniquement.
//!
//! - `pnex-device` : dernières valeurs des pins de N devices via OpenObserve
//!   (PromQL `last_over_time`, même série que l'ingestion) ;
//! - `pnex-calc` : expression sur les clés de payload (`pnex_core::eval_calc`,
//!   l'évaluateur partagé avec l'éditeur) ;
//! - `pnex-metric` : remote-write du résultat — série `etl_*` avec device
//!   virtuel `flow_{id}`, visible au catalogue Visualisation comme un capteur.
//!
//! L'org OpenObserve est estampillée dans l'artefact au deploy
//! (`pnex_org_id`), les creds racine viennent de la allowlist env du
//! superviseur — **aucun secret dans flows.json**.

pub mod calc;
pub mod device;
pub mod metric;
pub mod o2;

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
