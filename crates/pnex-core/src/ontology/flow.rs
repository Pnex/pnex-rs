//! Ontology flow nodes (partner-watch.md D312): `ontology_query`,
//! `object_upsert` and `link_upsert`. Their configs (shared by the editor
//! and the runtime) and the bodies of the internal routes the runtime
//! calls; the backend reads the node config from the *deployed* graph, so
//! a request only carries the values of the message. serde-only, wasm-safe.
//! No `flatten` in the bodies (serde_json `arbitrary_precision`).

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use super::api::{FilterOp, PropertyFilter, QUERY_LIMIT_MAX};
use super::schema::valid_key;

/// Filter values resolved by the runtime at each message: today's date
/// (`YYYY-MM-DD`, UTC), now (RFC 3339), or `$msg.<path>`.
pub const VALUE_TODAY: &str = "$today";
pub const VALUE_NOW: &str = "$now";
pub const VALUE_MSG_PREFIX: &str = "$msg.";

/// Writes accepted per org and per minute from the ontology nodes.
pub const WRITES_PER_MINUTE: u32 = 600;

/// One message per row, or one message holding every row.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum QueryOutput {
    #[default]
    Each,
    Array,
}

/// `ontology_query`: the objects of one type matching the filters.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct OntologyQueryConfig {
    #[serde(default)]
    pub object_type: String,
    #[serde(default)]
    pub filters: Vec<PropertyFilter>,
    /// Property to sort on (title by default).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub order_by: Option<String>,
    #[serde(default)]
    pub descending: bool,
    /// 1–500 (default 100).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<u64>,
    #[serde(default)]
    pub output: QueryOutput,
}

/// `object_upsert`: `msg.payload` = the properties, matched on `key` (a
/// unique property of the type) or on `msg.object_id` when `key` = `id`.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct ObjectUpsertConfig {
    #[serde(default)]
    pub object_type: String,
    #[serde(default)]
    pub key: String,
    /// An object already there is left as is (a rule must not reset a
    /// finding a human reviewed): only absent ones are created.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub skip_existing: bool,
}

/// `link_upsert`: opens (or updates the attributes of) the link
/// `msg.payload.source_id` → `msg.payload.target_id`, or closes it.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct LinkUpsertConfig {
    #[serde(default)]
    pub link_type: String,
    #[serde(default)]
    pub close: bool,
    /// A link of this pair closed before is answered as is, never opened
    /// again: a candidate a human rejected is not proposed twice
    /// (web-presence.md D362).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub skip_closed: bool,
}

fn bad(code: &'static str, message: &str) -> Option<(&'static str, String)> {
    Some((code, message.to_string()))
}

impl OntologyQueryConfig {
    /// Save validation: keys well formed, limit bounded. Existence in the
    /// schema is checked at deploy.
    pub fn check(&self) -> Option<(&'static str, String)> {
        let key_ok = |k: &str| k.is_empty() || valid_key(k);
        if !key_ok(&self.object_type)
            || self.filters.iter().any(|f| !valid_key(&f.key))
            || self.order_by.as_deref().is_some_and(|k| !valid_key(k))
        {
            return bad("ontology_query_invalid", "invalid type or property key");
        }
        if self
            .filters
            .iter()
            .any(|f| f.op == FilterOp::In && !f.value.is_array())
        {
            return bad("ontology_query_invalid", "`in` takes a list of values");
        }
        if self.limit.is_some_and(|l| l == 0 || l > QUERY_LIMIT_MAX) {
            return bad("ontology_query_invalid", "limit is 1 to 500");
        }
        None
    }

    pub fn check_deployable(&self) -> Option<(&'static str, String)> {
        if self.object_type.is_empty() {
            return bad("ontology_node_unset", "pick an object type");
        }
        self.check()
    }
}

impl ObjectUpsertConfig {
    pub fn check(&self) -> Option<(&'static str, String)> {
        let key_ok = |k: &str| k.is_empty() || k == "id" || valid_key(k);
        if !key_ok(&self.object_type) || !key_ok(&self.key) {
            return bad("object_upsert_invalid", "invalid type or key");
        }
        None
    }

    pub fn check_deployable(&self) -> Option<(&'static str, String)> {
        if self.object_type.is_empty() || self.key.is_empty() {
            return bad("ontology_node_unset", "pick an object type and its key");
        }
        self.check()
    }
}

impl LinkUpsertConfig {
    pub fn check(&self) -> Option<(&'static str, String)> {
        if !self.link_type.is_empty() && !valid_key(&self.link_type) {
            return bad("link_upsert_invalid", "invalid link type key");
        }
        None
    }

    pub fn check_deployable(&self) -> Option<(&'static str, String)> {
        if self.link_type.is_empty() {
            return bad("ontology_node_unset", "pick a link type");
        }
        self.check()
    }
}

/// Who calls: the deployed node (its config is read from that version).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FlowNodeRef {
    pub org_id: i64,
    pub flow_id: i64,
    pub version: i64,
    pub node_id: String,
}

/// `POST /internal/flow/ontology/query`: the filters with their values
/// resolved by the runtime (same keys and operators as the config).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FlowQueryRequest {
    pub node: FlowNodeRef,
    #[serde(default)]
    pub values: Vec<Value>,
}

/// `POST /internal/flow/ontology/upsert`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FlowUpsertRequest {
    pub node: FlowNodeRef,
    /// `msg.object_id` when the node's key is `id`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub object_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default)]
    pub properties: Map<String, Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected_version: Option<i32>,
}

/// `POST /internal/flow/ontology/link`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FlowLinkRequest {
    pub node: FlowNodeRef,
    pub source_id: String,
    pub target_id: String,
    #[serde(default)]
    pub attributes: Map<String, Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub valid_from: Option<String>,
}

/// Answer of an upsert: the object, and whether it was created.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FlowUpsertAck {
    pub created: bool,
    pub object: super::api::ObjectView,
}

/// Resolves the runtime value tokens of a filter value; `msg` reads a
/// path of the message (`None` = absent).
pub fn resolve_value(
    v: &Value,
    today: &str,
    now: &str,
    msg: &dyn Fn(&str) -> Option<Value>,
) -> Value {
    match v {
        Value::String(s) if s == VALUE_TODAY => Value::String(today.to_string()),
        Value::String(s) if s == VALUE_NOW => Value::String(now.to_string()),
        Value::String(s) if s.starts_with(VALUE_MSG_PREFIX) => {
            msg(&s[VALUE_MSG_PREFIX.len()..]).unwrap_or(Value::Null)
        }
        Value::Array(a) => Value::Array(
            a.iter()
                .map(|x| resolve_value(x, today, now, msg))
                .collect(),
        ),
        other => other.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn configs_are_checked() {
        let q: OntologyQueryConfig = serde_json::from_value(json!({
            "object_type": "procedure",
            "filters": [{"key": "prochaine_echeance", "op": "lte", "value": "$today"},
                        {"key": "etat", "op": "in", "value": ["ouverte"]}],
            "limit": 200
        }))
        .unwrap();
        assert!(q.check_deployable().is_none());
        let bad_in: OntologyQueryConfig = serde_json::from_value(json!({
            "object_type": "x", "filters": [{"key": "etat", "op": "in", "value": "ouverte"}]
        }))
        .unwrap();
        assert!(bad_in.check().is_some());
        assert_eq!(
            OntologyQueryConfig::default().check_deployable().unwrap().0,
            "ontology_node_unset"
        );
        let u = ObjectUpsertConfig {
            object_type: "ecart".into(),
            key: "id".into(),
            skip_existing: false,
        };
        assert!(u.check_deployable().is_none());
        assert!(LinkUpsertConfig {
            link_type: "Bad".into(),
            close: false,
            skip_closed: false,
        }
        .check()
        .is_some());
    }

    #[test]
    fn values_resolve_from_the_message() {
        let msg = |p: &str| (p == "payload.siren").then(|| json!("123456789"));
        let v = resolve_value(
            &json!(["$today", "$msg.payload.siren", 3]),
            "2026-10-19",
            "t",
            &msg,
        );
        assert_eq!(v, json!(["2026-10-19", "123456789", 3]));
        assert_eq!(
            resolve_value(&json!("$msg.nope"), "", "", &msg),
            Value::Null
        );
    }
}
