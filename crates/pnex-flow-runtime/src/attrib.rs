//! Attribution des événements debug aux nœuds éditeur — **par tab**.
//!
//! Le moteur identifie les nœuds par l'hex de `ElementId` (hash du id RED
//! quand il n'est pas hexadécimal — cf. `parse_red_id_str`). Avec la ferme
//! d'engines, l'attribution **flow** devient la clé de la ferme (chaque
//! pompe debug connaît son `flow_id`) ; il ne reste qu'une correspondance :
//! hex(nœud) → id éditeur (`"n2"`), reconstruite à la construction de chaque
//! engine depuis le tableau de son tab.

use std::collections::HashMap;

use serde_json::Value;

use edgelink_core::runtime::model::json::deser::parse_red_id_str;

/// hex(`ElementId` nœud) → id éditeur (`"n2"`) pour les entrées d'un tab
/// (celles qui portent `z`). L'id projeté `pnexflow{fid}_{raw}` est
/// dé-préfixé — le panneau debug se rattache aux ids de l'éditeur.
pub fn node_map(entries: &[Value]) -> HashMap<String, String> {
    let mut map = HashMap::new();
    for e in entries {
        let Some(id) = e.get("id").and_then(|v| v.as_str()) else {
            continue;
        };
        let Some(z) = e.get("z").and_then(|v| v.as_str()) else {
            continue;
        };
        let Some(hex) = parse_red_id_str(id).map(|eid| eid.to_string()) else {
            continue;
        };
        let raw = id.strip_prefix(&format!("{z}_")).unwrap_or(id);
        map.insert(hex, raw.to_string());
    }
    map
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn node_map_determine_le_hex_et_deprefixe() {
        // Ids projetés `pnexflow{fid}_{raw}` non hex : DefaultHasher
        // déterministe dans le process — parse_red_id_str retourne le même
        // hash que le moteur.
        let tab = json!([
            { "id": "pnexflow1_n1", "z": "pnexflow1", "type": "inject", "payload": "1" },
            { "id": "pnexflow1_n2", "z": "pnexflow1", "type": "debug", "tosidebar": true },
            { "id": "pnexflow1", "type": "tab", "label": "Flow #1" },
        ]);
        let m = node_map(tab.as_array().unwrap());
        let n2_hex = parse_red_id_str("pnexflow1_n2").unwrap().to_string();
        assert_eq!(m.get(&n2_hex).map(String::as_str), Some("n2"));
        // Le tab (sans z) n'entre pas dans la map.
        assert_eq!(m.len(), 2);
    }

    #[test]
    fn tab_vide_donne_une_map_vide() {
        assert!(node_map(&[json!({"id": "pnexflow1", "type": "tab"})]).is_empty());
        assert!(node_map(&[json!({"z": "x"}), json!({"id": "y"})]).is_empty());
    }
}
