//! Named queries of a pack (pipeline-engine.md D948): a read-only `SELECT`
//! with typed parameters, run like any analysis query (D825, D826) under
//! the caller's rights. A value never becomes SQL: it is checked against
//! its type, then written as a literal (a text quoted, its quotes doubled).

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Parameters of one query at most.
pub const PARAMS_MAX: usize = 12;
/// Longest text value.
pub const TEXT_MAX: usize = 500;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "snake_case")]
pub enum ParamKind {
    Text,
    Integer,
    Number,
    /// `YYYY-MM-DD`.
    Date,
    /// RFC 3339, written in UTC.
    Timestamp,
    Uuid,
    /// One of `values`.
    Choice,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(deny_unknown_fields)]
pub struct NamedParam {
    pub name: String,
    #[serde(rename = "type")]
    pub kind: ParamKind,
    /// `choice`: the values allowed.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub values: Vec<String>,
    /// Absent: the query reads `NULL`.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub optional: bool,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub description: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(deny_unknown_fields)]
pub struct NamedQuery {
    pub key: String,
    pub name: String,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub names: BTreeMap<String, String>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub description: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub params: Vec<NamedParam>,
    /// `SELECT … {{param}} …`.
    pub sql: String,
}

/// A run of a named query: its parameter values.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct NamedQueryRun {
    #[serde(default)]
    #[cfg_attr(feature = "openapi", schema(value_type = Object))]
    pub params: serde_json::Map<String, Value>,
    /// As in an analysis query: the caller's own recent run of the same
    /// text answers instead (live blocks).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_age_secs: Option<u32>,
}

/// A named query of an installed pack.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct NamedQueryView {
    pub pack: String,
    #[serde(flatten)]
    pub query: NamedQuery,
}

fn name_ok(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 64
        && s.starts_with(|c: char| c.is_ascii_lowercase())
        && s.chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
}

/// The names written `{{name}}` in a text.
fn placeholders(sql: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut rest = sql;
    while let Some(i) = rest.find("{{") {
        let after = &rest[i + 2..];
        let Some(j) = after.find("}}") else { break };
        out.push(after[..j].trim());
        rest = &after[j + 2..];
    }
    out
}

impl NamedQuery {
    /// What is wrong with a shipped query, as `field token`.
    pub fn violations(&self) -> Vec<String> {
        let mut out = Vec::new();
        if !name_ok(&self.key) {
            out.push("key invalid".into());
        }
        if self.name.trim().is_empty() || self.name.len() > 255 {
            out.push("name invalid".into());
        }
        if !self
            .sql
            .trim_start()
            .to_ascii_lowercase()
            .starts_with("select")
            && !self
                .sql
                .trim_start()
                .to_ascii_lowercase()
                .starts_with("with")
        {
            out.push("sql select_only".into());
        }
        if self.params.len() > PARAMS_MAX {
            out.push(format!("params max_items:{PARAMS_MAX}"));
        }
        for p in &self.params {
            if !name_ok(&p.name) {
                out.push(format!("params.{} invalid", p.name));
            }
            if (p.kind == ParamKind::Choice) == p.values.is_empty() {
                out.push(format!("params.{} values", p.name));
            }
        }
        for n in placeholders(&self.sql) {
            if !self.params.iter().any(|p| p.name == n) {
                out.push(format!("sql unknown_param:{n}"));
            }
        }
        out
    }

    /// The text with each `{{param}}` as a literal of its checked value.
    /// `Err((param, token))` on the first value refused.
    pub fn filled(
        &self,
        given: &serde_json::Map<String, Value>,
    ) -> Result<String, (String, String)> {
        for k in given.keys() {
            if !self.params.iter().any(|p| &p.name == k) {
                return Err((k.clone(), "unknown".into()));
            }
        }
        let mut literals = BTreeMap::new();
        for p in &self.params {
            let lit = match given.get(&p.name).filter(|v| !v.is_null()) {
                None if p.optional => "NULL".to_string(),
                None => return Err((p.name.clone(), "required".into())),
                Some(v) => literal(p, v).ok_or_else(|| (p.name.clone(), "invalid".into()))?,
            };
            literals.insert(p.name.as_str(), lit);
        }
        let mut out = String::with_capacity(self.sql.len());
        let mut rest = self.sql.as_str();
        while let Some(i) = rest.find("{{") {
            out.push_str(&rest[..i]);
            let after = &rest[i + 2..];
            let Some(j) = after.find("}}") else {
                out.push_str(&rest[i..]);
                rest = "";
                break;
            };
            let name = after[..j].trim();
            let lit = literals
                .get(name)
                .ok_or_else(|| (name.to_string(), "unknown".to_string()))?;
            out.push_str(lit);
            rest = &after[j + 2..];
        }
        out.push_str(rest);
        Ok(out)
    }
}

fn quoted(s: &str) -> String {
    format!("'{}'", s.replace('\'', "''"))
}

fn literal(p: &NamedParam, v: &Value) -> Option<String> {
    let text = match v {
        Value::String(s) => s.trim().to_string(),
        Value::Number(n) => n.to_string(),
        Value::Bool(b) => b.to_string(),
        _ => return None,
    };
    match p.kind {
        ParamKind::Text => (text.chars().count() <= TEXT_MAX
            && !text.chars().any(char::is_control))
        .then(|| quoted(&text)),
        ParamKind::Integer => text.parse::<i64>().ok().map(|n| n.to_string()),
        ParamKind::Number => text
            .parse::<f64>()
            .ok()
            .filter(|n| n.is_finite())
            .map(|n| format!("{n:?}")),
        ParamKind::Date => chrono::NaiveDate::parse_from_str(&text, "%Y-%m-%d")
            .ok()
            .map(|d| format!("DATE '{d}'")),
        ParamKind::Timestamp => chrono::DateTime::parse_from_rfc3339(&text).ok().map(|t| {
            format!(
                "TIMESTAMP '{}'",
                t.with_timezone(&chrono::Utc)
                    .format("%Y-%m-%dT%H:%M:%S%.6f")
            )
        }),
        ParamKind::Uuid => uuid::Uuid::parse_str(&text)
            .ok()
            .map(|u| quoted(&u.to_string())),
        ParamKind::Choice => p.values.contains(&text).then(|| quoted(&text)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn query() -> NamedQuery {
        serde_json::from_value(json!({
            "key": "bars", "name": "Bars",
            "params": [
                {"name": "instrument", "type": "text"},
                {"name": "at", "type": "timestamp"},
                {"name": "limit", "type": "integer", "optional": true},
                {"name": "side", "type": "choice", "values": ["buy", "sell"], "optional": true}
            ],
            "sql": "SELECT * FROM bars WHERE instrument = {{instrument}} AND ts <= {{at}} AND ({{side}} IS NULL OR side = {{side}}) LIMIT {{limit}}"
        }))
        .unwrap()
    }

    #[test]
    fn values_become_literals_never_sql() {
        let q = query();
        assert!(q.violations().is_empty(), "{:?}", q.violations());
        let sql = q
            .filled(
                json!({"instrument": "x' OR '1'='1", "at": "2026-01-02T03:04:05+01:00"})
                    .as_object()
                    .unwrap(),
            )
            .unwrap();
        assert!(sql.contains("instrument = 'x'' OR ''1''=''1'"), "{sql}");
        assert!(
            sql.contains("ts <= TIMESTAMP '2026-01-02T02:04:05.000000'"),
            "{sql}"
        );
        assert!(sql.ends_with("LIMIT NULL"), "{sql}");
        let bad = |v: Value| q.filled(v.as_object().unwrap()).unwrap_err();
        assert_eq!(bad(json!({"at": "2026-01-01T00:00:00Z"})).1, "required");
        assert_eq!(
            bad(json!({"instrument": "a", "at": "yesterday"})).1,
            "invalid"
        );
        assert_eq!(
            bad(json!({"instrument": "a", "at": "2026-01-01T00:00:00Z", "limit": "1; DROP"})).1,
            "invalid"
        );
        assert_eq!(
            bad(json!({"instrument": "a", "at": "2026-01-01T00:00:00Z", "side": "hold"})).1,
            "invalid"
        );
        assert_eq!(
            bad(json!({"instrument": "a", "at": "2026-01-01T00:00:00Z", "x": 1})).0,
            "x"
        );
    }

    #[test]
    fn a_query_names_only_its_params() {
        let mut q = query();
        q.sql = "DELETE FROM bars WHERE x = {{nope}}".into();
        let v = q.violations();
        assert!(v.contains(&"sql select_only".to_string()), "{v:?}");
        assert!(v.contains(&"sql unknown_param:nope".to_string()), "{v:?}");
    }
}
