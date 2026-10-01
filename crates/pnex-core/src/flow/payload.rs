//! Payload boundary helpers shared by validation and projection.

use super::*;

/// Map numérique extraite d'un payload (clés device sanitisées → valeur).
pub type NumericMap = std::collections::HashMap<String, f64>;

/// Contrat d'entrée du nœud `calc` / de sortie du nœud `device` : objet JSON
/// `clé → valeur numérique` (booléens convertis 1/0, parité avec
/// `handle_state_report`). Rejeté à la frontière — jamais de panic.
pub fn numeric_map_from_payload(
    payload: Option<&serde_json::Value>,
    node: &str,
) -> Result<NumericMap, FlowViolation> {
    let Some(serde_json::Value::Object(map)) = payload else {
        return Err(FlowViolation::new(
            None,
            &format!("{node}_input_contract"),
            format!(
                "le nœud {node} attend un objet JSON en payload (sortie d'un nœud device), reçu : {}",
                payload.map(type_of).unwrap_or("payload absent")
            ),
        ));
    };
    let mut out = NumericMap::with_capacity(map.len());
    for (key, value) in map {
        let v = match value {
            serde_json::Value::Number(n) => n.as_f64().unwrap_or(f64::NAN),
            serde_json::Value::Bool(b) => {
                if *b {
                    1.0
                } else {
                    0.0
                }
            }
            other => {
                return Err(FlowViolation::new(
                    None,
                    &format!("{node}_input_contract"),
                    format!(
                        "valeur non numérique pour la clé « {key} » (reçu : {})",
                        type_of(other)
                    ),
                ));
            }
        };
        out.insert(key.clone(), v);
    }
    Ok(out)
}

/// Contrat d'entrée du nœud `metric` : une valeur numérique (sortie d'un
/// nœud `calc`) — booléen converti 1/0, sinon rejet typé.
pub fn metric_value_from_payload(
    payload: Option<&serde_json::Value>,
) -> Result<f64, FlowViolation> {
    match payload {
        Some(serde_json::Value::Number(n)) => n.as_f64().ok_or_else(|| {
            FlowViolation::new(
                None,
                "metric_input_contract",
                "valeur numérique hors plage f64",
            )
        }),
        Some(serde_json::Value::Bool(b)) => Ok(if *b { 1.0 } else { 0.0 }),
        other => Err(FlowViolation::new(
            None,
            "metric_input_contract",
            format!(
                "le nœud metric attend une valeur numérique en payload (sortie d'un nœud calc), reçu : {}",
                other.map(type_of).unwrap_or("payload absent")
            ),
        )),
    }
}

fn type_of(v: &serde_json::Value) -> &'static str {
    match v {
        serde_json::Value::Null => "null",
        serde_json::Value::Bool(_) => "booléen",
        serde_json::Value::Number(_) => "nombre",
        serde_json::Value::String(_) => "chaîne",
        serde_json::Value::Array(_) => "tableau",
        serde_json::Value::Object(_) => "objet",
    }
}
