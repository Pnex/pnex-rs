//! Ontology flow nodes (partner-watch.md D312): `ontology_query`,
//! `object_upsert` and `link_upsert`. Their configs (shared by the editor
//! and the runtime) and the bodies of the internal routes the runtime
//! calls; the backend reads the node config from the *deployed* graph, so
//! a request only carries the values of the message. serde-only, wasm-safe.
//! No `flatten` in the bodies (serde_json `arbitrary_precision`).

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use super::api::{FilterOp, Hop, PropertyFilter, MAX_HOPS, QUERY_LIMIT_MAX};
use super::schema::valid_key;

/// Filter values resolved by the runtime at each message: today's date
/// (`YYYY-MM-DD`, UTC), now (RFC 3339), or `$msg.<path>`.
pub const VALUE_TODAY: &str = "$today";
pub const VALUE_NOW: &str = "$now";
pub const VALUE_MSG_PREFIX: &str = "$msg.";
/// Days around today accepted by `$today-<n>` / `$today+<n>` (crm.md D451).
pub const VALUE_DAYS_MAX: i64 = 3650;
/// `in` value resolved by the server: the codes of a pack setting
/// (`$setting.<pack>.<key>`); an empty setting drops the filter
/// (network-check.md D423).
pub const VALUE_SETTING_PREFIX: &str = "$setting.";

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
    /// Links followed from the selected objects; the rows are the objects
    /// reached (a certification's partners: `habilite_pour`, in).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub traverse: Vec<Hop>,
    /// Link types joined to each row, both directions, open links only
    /// (network-check.md D402).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub with_links: Vec<String>,
}

/// Link types an `ontology_query` may join to its rows.
pub const WITH_LINKS_MAX: usize = 8;
/// Links of one type joined to one row.
pub const LINKS_PER_ROW_MAX: usize = 200;

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
    /// A new link is a proposal waiting for a human (proposals.md D375);
    /// an open link still gets its attributes.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub propose: bool,
    /// Numeric attribute that only rises: an open link keeps its attributes
    /// when the message brings a lower value (a weaker candidate never
    /// degrades a confirmed link).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub keep_best: String,
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
            .any(|f| f.op == FilterOp::In && !f.value.is_array() && setting_of(&f.value).is_none())
        {
            return bad(
                "ontology_query_invalid",
                "`in` takes a list of values or a $setting.<pack>.<key>",
            );
        }
        if self.limit.is_some_and(|l| l == 0 || l > QUERY_LIMIT_MAX) {
            return bad("ontology_query_invalid", "limit is 1 to 500");
        }
        if self.traverse.len() > MAX_HOPS
            || self.traverse.iter().any(|h| !valid_key(&h.link_type))
            || self.with_links.len() > WITH_LINKS_MAX
            || self.with_links.iter().any(|l| !valid_key(l))
        {
            return bad(
                "ontology_query_invalid",
                "at most 4 traversal steps and 8 joined link types, with valid keys",
            );
        }
        None
    }

    /// Link types the node names (checked against the schema at deploy).
    pub fn link_types(&self) -> impl Iterator<Item = &String> {
        self.traverse
            .iter()
            .map(|h| &h.link_type)
            .chain(self.with_links.iter())
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
        if !self.keep_best.is_empty() && !valid_key(&self.keep_best) {
            return bad("link_upsert_invalid", "invalid keep_best attribute key");
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

/// Reply of `POST /internal/flow/ontology/query`.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct FlowQueryReply {
    pub count: i64,
    pub rows: Vec<FlowQueryRow>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FlowQueryRow {
    pub object: super::api::ObjectView,
    /// Link type → `[{direction, attributes, valid_from, other: {id, type,
    /// title, properties}}]`, for the node's `with_links`.
    #[serde(default, skip_serializing_if = "Map::is_empty")]
    pub links: Map<String, Value>,
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
    /// Confidence of a proposal (0..=100), from `msg.payload.confidence`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub confidence: Option<f64>,
    /// Evidence of a proposal (quotes, a model's opinion), from
    /// `msg.payload.evidence`.
    #[serde(default, skip_serializing_if = "Value::is_null")]
    pub evidence: Value,
    /// The org rule that accepts the proposal without a human
    /// (proposals.md D377), from `msg.payload.accept`; cited as the reason.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub accept: Option<String>,
}

/// Answer of an upsert: the object, and whether it was created.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FlowUpsertAck {
    pub created: bool,
    pub object: super::api::ObjectView,
}

/// The pack setting an `in` value names (`<pack>.<key>`), if it is one.
pub fn setting_of(v: &Value) -> Option<&str> {
    v.as_str()?.strip_prefix(VALUE_SETTING_PREFIX).filter(|k| {
        k.split_once('.')
            .is_some_and(|(a, b)| valid_key(a) && valid_key(b))
    })
}

/// The day offset of `$today-<n>` / `$today+<n>` (crm.md D451).
pub fn days_of(token: &str) -> Option<i64> {
    let rest = token.strip_prefix(VALUE_TODAY)?;
    let (sign, n) = match rest.split_at_checked(1)? {
        ("-", n) => (-1, n),
        ("+", n) => (1, n),
        _ => return None,
    };
    if n.is_empty() || !n.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    n.parse::<i64>()
        .ok()
        .filter(|d| *d <= VALUE_DAYS_MAX)
        .map(|d| sign * d)
}

/// `today` (`YYYY-MM-DD`) moved by `days`.
fn shift_date(today: &str, days: i64) -> Option<String> {
    let d = chrono::NaiveDate::parse_from_str(today, "%Y-%m-%d").ok()?;
    Some(
        d.checked_add_signed(chrono::Duration::days(days))?
            .format("%Y-%m-%d")
            .to_string(),
    )
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
        Value::String(s) if days_of(s).is_some() => {
            shift_date(today, days_of(s).unwrap_or(0)).map_or(Value::Null, Value::String)
        }
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
    fn today_offsets_resolve() {
        let none = |_: &str| None;
        let r = |v: &str| resolve_value(&json!(v), "2026-03-01", "", &none);
        assert_eq!(r("$today-14"), json!("2026-02-15"));
        assert_eq!(r("$today+1"), json!("2026-03-02"));
        assert_eq!(r("$today-0"), json!("2026-03-01"));
        // Not a token: kept as text.
        assert_eq!(r("$today-"), json!("$today-"));
        assert_eq!(r("$today-1d"), json!("$today-1d"));
        assert_eq!(r("$today*2"), json!("$today*2"));
        assert_eq!(days_of("$today-3651"), None);
        assert_eq!(days_of("$today+3650"), Some(3650));
    }

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
            propose: false,
            keep_best: String::new(),
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
