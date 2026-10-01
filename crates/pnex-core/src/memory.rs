//! Shared flow memory contract (Valkey) — `memory-write` / `memory-read`
//! flow nodes. Pure and wasm-safe (serde only): compiled for the frontend.
//!
//! School (same as [`crate::last_cache`]): the **writer** owns the lifetime
//! (TTL = garbage collector, the value disappears when nobody refreshes it),
//! the **reader** owns the freshness (max age checked against the write
//! timestamp, never against the remaining TTL). A missing, expired or stale
//! entry reads as `null` — never an invented value.
//!
//! Keys are scoped per organization: every flow of the org shares the same
//! memory space (a flow writes, any other flow reads).

use serde::{Deserialize, Serialize};

/// Max length of a memory key (user part).
pub const MEMORY_KEY_MAX_LEN: usize = 64;
/// Default lifetime of a written value (1 h).
pub const MEMORY_TTL_DEFAULT_SECS: u32 = 3600;
/// Upper bound of the lifetime (30 days) — memory is live state, not storage.
pub const MEMORY_TTL_MAX_SECS: u32 = 30 * 86_400;
/// Upper bound of the reader max age (same as the TTL bound).
pub const MEMORY_MAX_AGE_MAX_SECS: f64 = MEMORY_TTL_MAX_SECS as f64;
/// Max serialized size of one stored value.
pub const MEMORY_VALUE_MAX_BYTES: usize = 64 * 1024;
/// Max number of keys read by one node.
pub const MEMORY_READ_MAX_KEYS: usize = 32;

/// Key charset: `[A-Za-z0-9_.-]`, 1..=[`MEMORY_KEY_MAX_LEN`] characters.
/// Closed charset: no Valkey glob/separator surprise, readable in the UI.
pub fn valid_memory_key(key: &str) -> bool {
    !key.is_empty()
        && key.len() <= MEMORY_KEY_MAX_LEN
        && key
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '-'))
}

/// Valkey key of a memory entry: `pnex:mem:v1:{org_id}:{key}`.
pub fn memory_cache_key(org_id: i64, key: &str) -> String {
    format!("pnex:mem:v1:{org_id}:{key}")
}

/// One stored value, serialized as a JSON string.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MemoryEntry {
    /// Any JSON value (the incoming `msg.payload`).
    pub v: serde_json::Value,
    /// Write time, epoch milliseconds (runtime host clock).
    pub ts_ms: i64,
}

/// Freshness resolver: `max_age_secs <= 0` accepts any age (the TTL alone
/// bounds the lifetime). Inclusive bound; negative age (clock skew) counts
/// as fresh.
pub fn resolve_memory(
    entry: Option<MemoryEntry>,
    now_ms: i64,
    max_age_secs: f64,
) -> Option<MemoryEntry> {
    let e = entry?;
    if max_age_secs > 0.0 && (now_ms - e.ts_ms) as f64 > max_age_secs * 1000.0 {
        return None;
    }
    Some(e)
}

/// Configuration of the `memory-write` flow node: stores `msg.payload`
/// under `key` for `ttl_secs`, then passes the message through.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MemoryWriteConfig {
    #[serde(default)]
    pub key: String,
    #[serde(default = "default_ttl")]
    pub ttl_secs: u32,
}

fn default_ttl() -> u32 {
    MEMORY_TTL_DEFAULT_SECS
}

impl Default for MemoryWriteConfig {
    fn default() -> Self {
        Self {
            key: String::new(),
            ttl_secs: MEMORY_TTL_DEFAULT_SECS,
        }
    }
}

impl MemoryWriteConfig {
    /// Structural check shared by the save validation and the runtime build.
    pub fn check(&self) -> Option<(&'static str, String)> {
        if !valid_memory_key(&self.key) {
            return Some(("memory_key_invalid", key_rule(&self.key)));
        }
        if self.ttl_secs == 0 || self.ttl_secs > MEMORY_TTL_MAX_SECS {
            return Some((
                "memory_ttl_invalid",
                format!("lifetime must be between 1 and {MEMORY_TTL_MAX_SECS} seconds"),
            ));
        }
        None
    }
}

/// Configuration of the `memory-read` flow node: one read of every key per
/// incoming message. Port 0 = object `{key: value|null}`, then one port per
/// key (`payload` = value or `null`, `topic` = key).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct MemoryReadConfig {
    #[serde(default)]
    pub keys: Vec<String>,
    /// Max age of an accepted value, in seconds; `0` = any age.
    #[serde(default)]
    pub max_age_secs: f64,
}

impl MemoryReadConfig {
    /// Port count: the object port plus one port per key.
    pub fn port_count(&self) -> usize {
        1 + self.keys.len()
    }

    /// Structural check shared by the save validation and the runtime build.
    pub fn check(&self) -> Option<(&'static str, String)> {
        if self.keys.is_empty() {
            return Some(("memory_keys_empty", "add at least one key to read".into()));
        }
        if self.keys.len() > MEMORY_READ_MAX_KEYS {
            return Some((
                "memory_keys_too_many",
                format!("at most {MEMORY_READ_MAX_KEYS} keys per node"),
            ));
        }
        let mut seen = std::collections::BTreeSet::new();
        for k in &self.keys {
            if !valid_memory_key(k) {
                return Some(("memory_key_invalid", key_rule(k)));
            }
            if !seen.insert(k.as_str()) {
                return Some((
                    "memory_key_duplicate",
                    format!("key \"{k}\" is listed twice"),
                ));
            }
        }
        if !(self.max_age_secs.is_finite()
            && (0.0..=MEMORY_MAX_AGE_MAX_SECS).contains(&self.max_age_secs))
        {
            return Some((
                "memory_max_age_invalid",
                format!("max age must be between 0 and {MEMORY_TTL_MAX_SECS} seconds"),
            ));
        }
        None
    }
}

/// Reference to a numeric memory value, for dashboards: `key` + optional
/// `field` of an object value (empty = the value itself).
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct MemoryRef {
    pub key: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub field: String,
}

impl MemoryRef {
    /// Stable identifier in the dashboards live-values map (`metric|device`
    /// for telemetry, `mem:key` / `mem:key#field` for memory).
    pub fn series_key(&self) -> String {
        if self.field.is_empty() {
            format!("mem:{}", self.key)
        } else {
            format!("mem:{}#{}", self.key, self.field)
        }
    }

    /// Same charset as keys for both parts (the field may be empty).
    pub fn is_valid(&self) -> bool {
        valid_memory_key(&self.key) && (self.field.is_empty() || valid_memory_key(&self.field))
    }
}

/// Numeric view of a stored value: a number, a boolean (1/0) or a numeric
/// string; with `field`, the same rule on that property of an object.
pub fn memory_numeric(v: &serde_json::Value, field: &str) -> Option<f64> {
    let v = if field.is_empty() { v } else { v.get(field)? };
    match v {
        serde_json::Value::Number(n) => n.as_f64().filter(|x| x.is_finite()),
        serde_json::Value::Bool(b) => Some(if *b { 1.0 } else { 0.0 }),
        serde_json::Value::String(s) => s.trim().parse::<f64>().ok().filter(|x| x.is_finite()),
        _ => None,
    }
}

/// Numeric fields of a stored value (dashboard picker): `[""]` for a
/// scalar numeric value, the numeric property names of an object.
pub fn memory_numeric_fields(v: &serde_json::Value) -> Vec<String> {
    match v {
        serde_json::Value::Object(map) => map
            .iter()
            .filter(|(k, _)| valid_memory_key(k))
            .filter(|(k, _)| memory_numeric(v, k).is_some())
            .map(|(k, _)| k.clone())
            .collect(),
        other if memory_numeric(other, "").is_some() => vec![String::new()],
        _ => Vec::new(),
    }
}

/// One live key of the org (`GET /api/v1/memory/keys`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MemoryKeyInfo {
    pub key: String,
    pub ts_ms: i64,
    /// Numeric fields (see [`memory_numeric_fields`]).
    #[serde(default)]
    pub fields: Vec<String>,
}

/// `POST /api/v1/memory/values` body.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MemoryValuesRequest {
    pub refs: Vec<MemoryRef>,
}

/// One resolved reference, same order as the request.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MemoryValue {
    pub available: bool,
    #[serde(default)]
    pub value: Option<f64>,
    #[serde(default)]
    pub ts_ms: Option<i64>,
}

/// `POST /api/v1/memory/values` response. `available: false` = memory
/// store unreachable/disabled (every result then unavailable).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MemoryValuesResponse {
    pub available: bool,
    pub results: Vec<MemoryValue>,
}

/// Max refs per `POST /api/v1/memory/values`.
pub const MEMORY_VALUES_CAP: usize = 64;

/// Compact duration label (`90 s`, `5 min`, `1 h`, `7 d`): the largest
/// unit dividing the value exactly.
pub fn format_duration_secs(secs: u32) -> String {
    match secs {
        0 => "0 s".into(),
        s if s % 86_400 == 0 => format!("{} d", s / 86_400),
        s if s % 3600 == 0 => format!("{} h", s / 3600),
        s if s % 60 == 0 => format!("{} min", s / 60),
        s => format!("{s} s"),
    }
}

fn key_rule(key: &str) -> String {
    format!(
        "key \"{key}\" must be 1 to {MEMORY_KEY_MAX_LEN} characters among letters, digits, '_', '.', '-'"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: i64 = 1_789_000_000_000;

    fn entry(age_ms: i64) -> Option<MemoryEntry> {
        Some(MemoryEntry {
            v: serde_json::json!({"t": 21.5}),
            ts_ms: NOW - age_ms,
        })
    }

    #[test]
    fn keys() {
        assert!(valid_memory_key("boiler.temp_1-a"));
        assert!(!valid_memory_key(""));
        assert!(!valid_memory_key("a b"));
        assert!(!valid_memory_key("a:b"));
        assert!(!valid_memory_key("a*"));
        assert!(!valid_memory_key(&"a".repeat(MEMORY_KEY_MAX_LEN + 1)));
        assert_eq!(memory_cache_key(7, "x"), "pnex:mem:v1:7:x");
        assert_eq!(format_duration_secs(90), "90 s");
        assert_eq!(format_duration_secs(300), "5 min");
        assert_eq!(format_duration_secs(3600), "1 h");
        assert_eq!(format_duration_secs(7 * 86_400), "7 d");
    }

    #[test]
    fn freshness() {
        assert!(resolve_memory(None, NOW, 0.0).is_none());
        assert!(
            resolve_memory(entry(10_000_000), NOW, 0.0).is_some(),
            "0 = any age"
        );
        assert!(
            resolve_memory(entry(5_000), NOW, 5.0).is_some(),
            "inclusive bound"
        );
        assert!(resolve_memory(entry(5_001), NOW, 5.0).is_none());
        assert!(
            resolve_memory(entry(-2_000), NOW, 1.0).is_some(),
            "skew = fresh"
        );
    }

    #[test]
    fn entry_roundtrip() {
        let e = entry(0).unwrap();
        let back: MemoryEntry = serde_json::from_str(&serde_json::to_string(&e).unwrap()).unwrap();
        assert_eq!(back.ts_ms, e.ts_ms);
        assert_eq!(back.v["t"].as_f64(), Some(21.5));
    }

    #[test]
    fn write_check() {
        let ok = MemoryWriteConfig {
            key: "k".into(),
            ..Default::default()
        };
        assert!(ok.check().is_none());
        assert_eq!(
            MemoryWriteConfig::default().check().unwrap().0,
            "memory_key_invalid"
        );
        let bad = MemoryWriteConfig {
            ttl_secs: 0,
            ..ok.clone()
        };
        assert_eq!(bad.check().unwrap().0, "memory_ttl_invalid");
        let bad = MemoryWriteConfig {
            ttl_secs: MEMORY_TTL_MAX_SECS + 1,
            ..ok
        };
        assert_eq!(bad.check().unwrap().0, "memory_ttl_invalid");
    }

    #[test]
    fn numeric_views() {
        let v = serde_json::json!({"Hmass": 412.5, "phase": "gas", "on": true, "s": "3.5"});
        assert_eq!(memory_numeric(&v, "Hmass"), Some(412.5));
        assert_eq!(memory_numeric(&v, "on"), Some(1.0));
        assert_eq!(memory_numeric(&v, "s"), Some(3.5));
        assert_eq!(memory_numeric(&v, "phase"), None);
        assert_eq!(memory_numeric(&v, ""), None);
        assert_eq!(memory_numeric(&serde_json::json!(7), ""), Some(7.0));
        let mut fields = memory_numeric_fields(&v);
        fields.sort();
        assert_eq!(fields, ["Hmass", "on", "s"]);
        assert_eq!(memory_numeric_fields(&serde_json::json!(7)), [""]);
        let r = MemoryRef {
            key: "cycle.p1".into(),
            field: "Hmass".into(),
        };
        assert_eq!(r.series_key(), "mem:cycle.p1#Hmass");
        assert!(r.is_valid());
    }

    #[test]
    fn read_check() {
        let cfg = |keys: &[&str], age: f64| MemoryReadConfig {
            keys: keys.iter().map(|s| s.to_string()).collect(),
            max_age_secs: age,
        };
        assert!(cfg(&["a", "b"], 0.0).check().is_none());
        assert_eq!(cfg(&["a", "b"], 0.0).port_count(), 3);
        assert_eq!(cfg(&[], 0.0).check().unwrap().0, "memory_keys_empty");
        assert_eq!(
            cfg(&["a", "a"], 0.0).check().unwrap().0,
            "memory_key_duplicate"
        );
        assert_eq!(cfg(&["a b"], 0.0).check().unwrap().0, "memory_key_invalid");
        assert_eq!(
            cfg(&["a"], -1.0).check().unwrap().0,
            "memory_max_age_invalid"
        );
        assert_eq!(
            cfg(&["a"], f64::NAN).check().unwrap().0,
            "memory_max_age_invalid"
        );
    }
}
