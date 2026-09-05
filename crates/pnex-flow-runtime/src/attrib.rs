//! Attribution des événements debug aux flows/nœuds éditeur.
//!
//! Le moteur identifie les nœuds par l'hex de `ElementId` (hash du id RED
//! quand il n'est pas hexadécimal — cf. `parse_red_id_str`) et les flows par
//! l'hex du tab — aucun des deux n'est mappable par le backend. Le runtime,
//! seul à posséder le `flows.json` et le même désérialiseur que le moteur,
//! reconstruit la correspondance au boot et à chaque redéploiement :
//! hex(tab) → `pnex_flow_id`, hex(nœud) → id éditeur (`"n2"`).

use std::collections::HashMap;

use serde_json::Value;

use edgelink_core::runtime::model::json::deser::parse_red_id_str;

/// Correspondance hex moteur ↔ identifiants éditeur, reconstruite depuis le
/// `flows.json` (fonctions pures, testable sans moteur).
#[derive(Debug, Default, Clone, PartialEq)]
pub struct Attribution {
    /// hex(`ElementId` du tab) → `pnex_flow_id`.
    flow_by_path: HashMap<String, i64>,
    /// hex(`ElementId` du nœud) → id éditeur (`"n2"`).
    node_by_hex: HashMap<String, String>,
}

impl Attribution {
    pub fn empty() -> Self {
        Self::default()
    }

    /// Charge l'artefact et construit les maps (best-effort : fichier
    /// illisible → maps vides, les événements non attribuables seront jetés
    /// côté backend plutôt que mal attribués).
    pub async fn load(flows_path: &str) -> Self {
        match tokio::fs::read_to_string(flows_path).await {
            Ok(s) => match serde_json::from_str(&s) {
                Ok(v) => Self::build(&v),
                Err(e) => {
                    log::warn!("flows.json illisible pour l'attribution debug : {e}");
                    Self::default()
                }
            },
            Err(e) => {
                log::warn!("flows.json absent pour l'attribution debug : {e}");
                Self::default()
            }
        }
    }

    /// Parse l'artefact : tabs `pnexflow(\d+)` → hex tab ; entrées `id`+`z`
    /// → hex nœud → id éditeur.
    pub fn build(json: &Value) -> Self {
        let mut a = Self::default();
        let Some(entries) = json.as_array() else { return a };
        for e in entries {
            let Some(id) = e.get("id").and_then(|v| v.as_str()) else { continue };
            let Some(hex) = parse_red_id_str(id).map(|eid| eid.to_string()) else { continue };
            if e.get("type").and_then(|t| t.as_str()) == Some("tab") {
                // Pas de dépendance regex : "pnexflow12" → 12.
                if let Some(fid) = id.strip_prefix("pnexflow").and_then(|s| s.parse::<i64>().ok()) {
                    a.flow_by_path.insert(hex, fid);
                }
                continue;
            }
            if e.get("z").is_some() {
                a.node_by_hex.insert(hex, id.to_string());
            }
        }
        a
    }

    /// Flow d'un `DebugMessage.path` (peut être `"<tab>/<subflow>"` — on
    /// prend le segment tab).
    pub fn flow_of_path(&self, path: &str) -> Option<i64> {
        let first = path.split('/').next().unwrap_or(path);
        self.flow_by_path.get(first).copied()
    }

    /// Id éditeur d'un id moteur (`m.id` hex ou déjà brut pour `pnex-display`
    /// — fallback identité).
    pub fn node_red(&self, hex_or_raw: &str) -> String {
        self.node_by_hex
            .get(hex_or_raw)
            .cloned()
            .unwrap_or_else(|| hex_or_raw.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn build_attribution_tabs_et_noeuds() {
        // "pnexflow1"/"n2" ne sont pas hex : DefaultHasher déterministe dans
        // le process — parse_red_id_str retourne le même hash que le moteur.
        let artefact = json!([
            {"id": "pnexflow1", "type": "tab", "label": "Flow #1 v1", "pnex_flow_id": 1},
            {"id": "n1", "z": "pnexflow1", "type": "inject", "payload": "1"},
            {"id": "n2", "z": "pnexflow1", "type": "debug", "tosidebar": true},
        ]);
        let a = Attribution::build(&artefact);
        let tab_hex = parse_red_id_str("pnexflow1").unwrap().to_string();
        let n2_hex = parse_red_id_str("n2").unwrap().to_string();
        assert_eq!(a.flow_of_path(&tab_hex), Some(1));
        // Segments de subflow : seul le tab compte.
        assert_eq!(a.flow_of_path(&format!("{tab_hex}/abcdef")), Some(1));
        assert_eq!(a.flow_of_path("inconnu"), None);
        assert_eq!(a.node_red(&n2_hex), "n2");
        // Déjà brut (nœud pnex-display qui s'identifie lui-même) → identité.
        assert_eq!(a.node_red("n2"), "n2");
        assert_eq!(a.node_red("inconnu"), "inconnu");
    }

    #[test]
    fn artefact_invalide_donne_des_maps_vides() {
        assert_eq!(Attribution::build(&json!(null)), Attribution::empty());
        assert_eq!(
            Attribution::build(&json!([{"z": "x"}, {"id": "y"}])),
            Attribution::empty()
        );
    }
}
