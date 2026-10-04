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

/// Max number of series one object payload writes through `pnex-metric`.
pub const METRIC_OBJECT_MAX_FIELDS: usize = 200;

/// Values the `metric` node writes for a payload (D140): a number or a
/// boolean = one series named after the node (`None` suffix); an object =
/// one series per top-level numeric / boolean field (suffix = the field,
/// sanitized by the caller through `etl_metric_name`), other fields
/// skipped. An object without any numeric field is refused.
pub fn metric_values_from_payload(
    payload: Option<&serde_json::Value>,
) -> Result<Vec<(Option<String>, f64)>, FlowViolation> {
    let Some(serde_json::Value::Object(map)) = payload else {
        return metric_value_from_payload(payload).map(|v| vec![(None, v)]);
    };
    let out: Vec<(Option<String>, f64)> = map
        .iter()
        .filter_map(|(k, v)| {
            match v {
                serde_json::Value::Number(n) => n.as_f64().filter(|x| x.is_finite()),
                serde_json::Value::Bool(b) => Some(if *b { 1.0 } else { 0.0 }),
                _ => None,
            }
            .map(|x| (Some(k.clone()), x))
        })
        .take(METRIC_OBJECT_MAX_FIELDS)
        .collect();
    if out.is_empty() {
        return Err(FlowViolation::new(
            None,
            "metric_input_contract",
            "the metric node expects a number, a boolean or an object with numeric fields",
        ));
    }
    Ok(out)
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
