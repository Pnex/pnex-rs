//! Generated Tier 2 snippets: platformio.ini (secrets b64 inline) + sketch
//! (declared pins, zero secrets), plus the key/value rows to JSON helper.

/// Lignes clé/valeur → objet JSON (lignes vides ignorées, `None` si vide).
pub(super) fn rows_to_json(rows: &[(String, String)]) -> Option<serde_json::Value> {
    let map: serde_json::Map<String, serde_json::Value> = rows
        .iter()
        .filter(|(k, v)| !k.trim().is_empty() && !v.trim().is_empty())
        .map(|(k, v)| {
            (
                k.trim().to_string(),
                serde_json::Value::String(v.trim().to_string()),
            )
        })
        .collect();
    if map.is_empty() {
        None
    } else {
        Some(serde_json::Value::Object(map))
    }
}
