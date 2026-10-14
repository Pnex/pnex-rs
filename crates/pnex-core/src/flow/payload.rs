//! Payload boundary helpers shared by validation and projection.

use super::*;

/// Map numérique extraite d'un payload (clés device sanitisées → valeur).
pub type NumericMap = std::collections::HashMap<String, f64>;

/// Input contract of the `calc` node: a JSON object whose numeric keys are
/// the variables (booleans as 1/0, parity with `handle_state_report`).
/// Non-numeric keys (arrays, text, nested objects — e.g. `detections` of a
/// vision node) are skipped, so `count > 0` works on any rich payload; an
/// expression naming a skipped key fails as an unknown variable (O66).
/// A non-object payload is rejected at the boundary — never a panic.
pub fn numeric_map_from_payload(
    payload: Option<&serde_json::Value>,
    node: &str,
) -> Result<NumericMap, FlowViolation> {
    let Some(serde_json::Value::Object(map)) = payload else {
        return Err(FlowViolation::new(
            None,
            &format!("{node}_input_contract"),
            format!(
                "the {node} node expects a JSON object payload (e.g. a device read), got: {}",
                payload.map(type_of).unwrap_or("no payload")
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
            _ => continue,
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
                "the metric node expects a numeric payload (e.g. a calc output), got: {}",
                other.map(type_of).unwrap_or("no payload")
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
        serde_json::Value::Bool(_) => "boolean",
        serde_json::Value::Number(_) => "number",
        serde_json::Value::String(_) => "text",
        serde_json::Value::Array(_) => "array",
        serde_json::Value::Object(_) => "object",
    }
}
