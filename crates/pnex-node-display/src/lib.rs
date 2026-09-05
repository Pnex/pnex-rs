//! Nœud de flow PNEX **sonde** (`pnex-display`) pour le runtime EdgeLinkd
//! headless — même convention que [`pnex_node_sql`] / [`pnex_node_device`] :
//! enregistré par `inventory` (macro `#[flow_node]`), build typé, secrets
//! jamais dans flows.json.
//!
//! - passthrough **intact** : le message continue vers l'aval inchangé ;
//! - publication au **canal debug** du moteur avec l'id canvas **brut**
//!   (`pnex_node_id` estampillé par la projection — `"n3"`) : c'est la clé
//!   de rattachement du badge live de l'éditeur et du panneau de debug ;
//! - valeur capturée = `msg.payload` si présent, sinon le message entier ;
//!   la valeur est publiée **brute** (le debug builtin pré-stringifie).
//!
//! L'identité est estampillée par la projection au deploy — jamais lue
//! d'une config client (anti-forgery d'attribution) ; le backend n'expose
//! de toute façon que ce que le runtime a attribué au tab (`flow`).

pub mod display;

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
