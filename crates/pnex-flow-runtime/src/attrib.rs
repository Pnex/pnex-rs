//! Attribution of debug events to editor nodes — **per tab**.
//!
//! The engine identifies nodes by the hex of `ElementId` (hash of the RED id
//! when it is not hexadecimal — see `parse_red_id_str`). With the engine
//! farm, **flow** attribution becomes the farm key (each
//! debug pump knows its `flow_id`); only one mapping remains:
//! hex(node) → editor id (`"n2"`), rebuilt when each
//! engine is built from its tab's array.

use std::collections::HashMap;

use serde_json::Value;

use edgelink_core::runtime::model::json::deser::parse_red_id_str;

/// hex(node `ElementId`) → editor id (`"n2"`) for the entries of a tab
/// (those carrying `z`). The projected id `pnexflow{fid}_{raw}` is
/// de-prefixed — the debug panel attaches to the editor ids.
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
    fn node_map_determines_the_hex_and_strips_prefix() {
        // Non-hex projected ids `pnexflow{fid}_{raw}`: DefaultHasher
        // deterministic within the process — parse_red_id_str returns the same
        // hash as the engine.
        let tab = json!([
            { "id": "pnexflow1_n1", "z": "pnexflow1", "type": "inject", "payload": "1" },
            { "id": "pnexflow1_n2", "z": "pnexflow1", "type": "debug", "tosidebar": true },
            { "id": "pnexflow1", "type": "tab", "label": "Flow #1" },
        ]);
        let m = node_map(tab.as_array().unwrap());
        let n2_hex = parse_red_id_str("pnexflow1_n2").unwrap().to_string();
        assert_eq!(m.get(&n2_hex).map(String::as_str), Some("n2"));
        // The tab (without z) does not go into the map.
        assert_eq!(m.len(), 2);
    }

    #[test]
    fn empty_tab_gives_an_empty_map() {
        assert!(node_map(&[json!({"id": "pnexflow1", "type": "tab"})]).is_empty());
        assert!(node_map(&[json!({"z": "x"}), json!({"id": "y"})]).is_empty());
    }
}
