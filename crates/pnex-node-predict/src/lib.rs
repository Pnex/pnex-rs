//! Predictive telemetry flow nodes (ml-vision.md step 3, roadmap P2.3):
//! `pnex-anomaly` (robust z / forecast band / changepoint) and
//! `pnex-forecast` (ETS-MSTL / linear trend, threshold breach ETA).
//!
//! Input contract (both nodes): one numeric series per `msg.topic` — a
//! scalar payload (booleans → 1/0), or the `key` field of an object
//! payload. Non-numeric input is rejected without panic. The algorithms are
//! pure ([`algo`]); windows and persistence live in [`series`].

pub mod algo;
mod anomaly;
mod forecast;
pub mod series;

use edgelink_core::runtime::model::*;
use edgelink_core::{EdgelinkError, Result};
use serde::Deserialize;

/// Anti-stripping anchor referenced by the `pnex-flow-runtime` binary: keeps
/// this crate's `inventory` submissions at link time.
pub fn registered() {}

/// Deploy metadata stamped by the projection (series persistence key).
#[derive(Debug, Default, Deserialize)]
struct ArtifactMeta {
    #[serde(default)]
    pnex_node_id: String,
    #[serde(default)]
    pnex_flow_id: i64,
    #[serde(default)]
    pnex_org_id: i64,
}

/// Numeric value of a scalar (booleans → 1/0).
fn scalar_of(v: &serde_json::Value) -> Option<f64> {
    match v {
        serde_json::Value::Number(n) => n.as_f64().filter(|x| x.is_finite()),
        serde_json::Value::Bool(b) => Some(if *b { 1.0 } else { 0.0 }),
        _ => None,
    }
}

/// Topic + numeric value of an incoming message.
async fn read_input(node: &str, msg: &MsgHandle, key: &str) -> Result<(String, f64)> {
    let m = msg.read().await;
    let topic = m
        .get("topic")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let payload = match m.get("payload") {
        Some(v) => serde_json::to_value(v).map_err(|e| {
            EdgelinkError::InvalidOperation(format!("{node} : payload not serializable : {e}"))
        })?,
        None => serde_json::Value::Null,
    };
    let value = match (&payload, key.is_empty()) {
        (serde_json::Value::Object(map), false) => {
            map.get(key).and_then(scalar_of).ok_or_else(|| {
                EdgelinkError::InvalidOperation(format!(
                    "{node} : payload field \"{key}\" is missing or not numeric"
                ))
            })?
        }
        (p, _) => scalar_of(p).ok_or_else(|| {
            let hint = if key.is_empty() {
                " (set the key field for object payloads)"
            } else {
                ""
            };
            EdgelinkError::InvalidOperation(format!("{node} : expected a numeric payload{hint}"))
        })?,
    };
    Ok((topic, value))
}

fn to_variant(node: &str, v: serde_json::Value) -> Result<Variant> {
    serde_json::from_value(v).map_err(|e| {
        EdgelinkError::InvalidOperation(format!("{node} : result not convertible : {e}")).into()
    })
}

/// Builds the port-0 envelope (the incoming msg, payload replaced) and one
/// fresh msg per extra `(port, value)`, all tagged with the series topic.
/// Ports beyond the artifact's wires array are skipped.
async fn envelopes(
    node: &str,
    msg: MsgHandle,
    topic: &str,
    detail: serde_json::Value,
    extra: Vec<(usize, serde_json::Value)>,
    port_count: usize,
) -> Result<smallvec::SmallVec<[Envelope; 4]>> {
    let mut out = smallvec::SmallVec::new();
    msg.write()
        .await
        .set("payload".to_string(), to_variant(node, detail)?);
    out.push(Envelope { port: 0, msg });
    for (port, value) in extra {
        if port >= port_count {
            continue;
        }
        let mut m = Msg::default();
        m.set("payload".to_string(), to_variant(node, value)?);
        if !topic.is_empty() {
            m.set("topic".to_string(), Variant::String(topic.to_string()));
        }
        out.push(Envelope {
            port,
            msg: MsgHandle::new(m),
        });
    }
    Ok(out)
}
