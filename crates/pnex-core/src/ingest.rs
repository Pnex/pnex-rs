//! The journal of the steps a pipeline plays for an import (its run
//! history). The imports' own `steps:` (pipelines.md D344) are retired: an
//! org's pipeline fills a Table, its import reads it. Serde-only, wasm-safe.

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Serialized size cap of a manifest (run history).
pub const MANIFEST_MAX_BYTES: usize = 64 * 1024;
/// Bytes one download or connector read keeps at most.
pub const DOWNLOAD_MAX_BYTES: u64 = 500 * 1024 * 1024;
/// Age after which a run's staging objects are purged.
pub const STAGING_TTL_DAYS: i64 = 7;

/// One journaled step of an import run (its run history).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct IngestStepView {
    #[cfg_attr(feature = "openapi", schema(format = "uuid"))]
    pub id: String,
    /// One id per pass of the chain.
    #[cfg_attr(feature = "openapi", schema(format = "uuid"))]
    pub run_id: String,
    pub node_id: String,
    pub flow_version: i64,
    /// The step of a pipeline run for the import (`fetch`, `read`…), or a
    /// step of the retired import steps (D344) in an older journal.
    pub step: String,
    /// 0 done, 1 error, 2 unchanged.
    pub port: u32,
    /// Machine code of an error (`err-<code>` in the UI).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub code: Option<String>,
    /// What the step received and produced (64 KB at most).
    #[cfg_attr(feature = "openapi", schema(value_type = Object))]
    pub manifest: Value,
    /// Staging bytes the step wrote.
    pub bytes: u64,
    /// Its staging files are deleted.
    pub purged: bool,
    #[cfg_attr(feature = "openapi", schema(format = DateTime))]
    pub created_at: String,
}

/// A manifest cut to [`MANIFEST_MAX_BYTES`]: arrays of an oversized object
/// are truncated (with `truncated: true`) until it fits.
pub fn bounded_manifest(mut v: Value) -> Value {
    let size = |v: &Value| serde_json::to_vec(v).map_or(usize::MAX, |b| b.len());
    while size(&v) > MANIFEST_MAX_BYTES {
        let Some(obj) = v.as_object_mut() else {
            return serde_json::json!({ "truncated": true });
        };
        let longest = obj
            .iter()
            .filter_map(|(k, x)| x.as_array().map(|a| (k.clone(), a.len())))
            .max_by_key(|(_, n)| *n);
        match longest {
            Some((k, n)) if n > 1 => {
                if let Some(a) = obj.get_mut(&k).and_then(Value::as_array_mut) {
                    a.truncate(n / 2);
                }
                obj.insert("truncated".into(), Value::Bool(true));
            }
            _ => return serde_json::json!({ "truncated": true }),
        }
    }
    v
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn manifest_is_bounded() {
        let big: Vec<Value> = (0..20_000)
            .map(|i| Value::String(format!("entry-{i}")))
            .collect();
        let m = bounded_manifest(serde_json::json!({ "rows": 3, "entries": big }));
        assert!(serde_json::to_vec(&m).unwrap().len() <= MANIFEST_MAX_BYTES);
        assert_eq!(m["truncated"], true);
        assert_eq!(m["rows"], 3);
    }
}
