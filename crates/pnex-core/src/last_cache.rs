//! Live last-value telemetry cache contract, shared by the backend write
//! side and the flow-runtime read side (`pnex-device-read`). Pure and
//! wasm-safe (serde only — no chrono, no std::time): `pnex-core` is compiled
//! for `wasm32-unknown-unknown` by the frontend check.
//!
//! School: OpenObserve remains the storage of record; the cache is an
//! accelerator that bridges the ingestion batcher latency (10 s default).
//! The TTL is a garbage collector only — freshness is decided by
//! [`resolve`] against the sample timestamp, never by expiry.

use serde::{Deserialize, Serialize};

/// TTL of a cached sample, in seconds. GC ONLY: it deliberately outlives the
/// largest legal freshness window (3600 s, enforced by the node build
/// validation) so the cache never answers differently from the OpenObserve
/// path purely because of expiry. 3600 s + 5 min headroom.
pub const LAST_SAMPLE_TTL_SECS: i64 = 3900;

/// One cached telemetry sample, serialized as a JSON string value.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct CachedSample {
    pub v: f64,
    /// Epoch milliseconds, server-receipt clock (same domain as the runtime
    /// host clock — supervised child on the same machine).
    pub ts_ms: i64,
}

/// Key of a last-value cache entry: `pnex:last:v1:{org_id}:{device_id}:{metric}`.
///
/// `metric` must already be normalized by the caller (`normalize_measurement_name`,
/// feature `naming`) — as done by ingestion and by the reader node. The key
/// builder still routes it through [`crate::naming::sanitize_metric_name`] as
/// an idempotent safety net: BOTH sides call this function, so keys cannot
/// drift. `device_id` is used raw (slug, closed charset per
/// `valid_device_label` — Valkey keys are exact-matched, no injection surface).
pub fn last_cache_key(org_id: i64, device_id: &str, metric: &str) -> String {
    format!(
        "pnex:last:v1:{org_id}:{}:{}",
        device_id,
        crate::naming::sanitize_metric_name(metric)
    )
}

/// TTL of an edge agent sample whose key is NOT recorded in OpenObserve
/// (D95): there is no O2 fallback for those keys, so the live cache is the
/// only copy of the last value — kept one week (still a GC: freshness is
/// decided by [`resolve`], never by expiry).
pub const AGENT_LIVE_ONLY_TTL_SECS: i64 = 7 * 24 * 3600;

/// Last non-numeric value (text or JSON object/array) of an edge agent key
/// (D95) — sibling of [`CachedSample`], which stays f64-only so the numeric
/// readers are untouched.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CachedJsonSample {
    pub v: serde_json::Value,
    pub ts_ms: i64,
}

/// Key of a non-numeric last-value entry:
/// `pnex:lastj:v1:{org_id}:{device_id}:{metric}` (same sanitation as
/// [`last_cache_key`]).
pub fn last_json_cache_key(org_id: i64, device_id: &str, metric: &str) -> String {
    format!(
        "pnex:lastj:v1:{org_id}:{}:{}",
        device_id,
        crate::naming::sanitize_metric_name(metric)
    )
}

/// Freshness resolver — pure, shared by the runtime node and the tests.
///
/// - `None` sample (miss, expired, undecodable) → `None`.
/// - Negative age (clock skew) counts as fresh.
/// - Inclusive window (`age == window` is fresh), mirroring the
///   `last_over_time(...[w s])` semantics of the legacy O2 path.
pub fn resolve(sample: Option<CachedSample>, now_ms: i64, window_secs: f64) -> Option<f64> {
    let s = sample?;
    let age_ms = (now_ms - s.ts_ms) as f64;
    if age_ms <= window_secs * 1000.0 {
        Some(s.v)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW_MS: i64 = 1_789_000_000_000;

    fn sample(age_ms: i64) -> Option<CachedSample> {
        Some(CachedSample {
            v: 42.5,
            ts_ms: NOW_MS - age_ms,
        })
    }

    #[test]
    fn serde_roundtrip() {
        let s = CachedSample { v: -3.25, ts_ms: 7 };
        let json = serde_json::to_string(&s).unwrap();
        let back: CachedSample = serde_json::from_str(&json).unwrap();
        assert_eq!(back, s);
    }

    #[test]
    fn resolve_fresh_within_window() {
        assert_eq!(resolve(sample(1500), NOW_MS, 2.0), Some(42.5));
    }

    #[test]
    fn resolve_stale_beyond_window() {
        assert_eq!(resolve(sample(5000), NOW_MS, 2.0), None);
    }

    #[test]
    fn resolve_missing_sample_is_none() {
        assert_eq!(resolve(None, NOW_MS, 2.0), None);
    }

    #[test]
    fn resolve_negative_age_is_fresh() {
        // Sample stamped slightly in the future (clock skew) = fresh.
        assert_eq!(resolve(sample(-500), NOW_MS, 2.0), Some(42.5));
    }

    #[test]
    fn resolve_window_boundary_is_inclusive() {
        assert_eq!(resolve(sample(2000), NOW_MS, 2.0), Some(42.5));
    }

    #[test]
    fn json_sample_key_and_roundtrip() {
        assert_eq!(
            last_json_cache_key(7, "agent-1", "status"),
            "pnex:lastj:v1:7:agent-1:status"
        );
        let s = CachedJsonSample {
            v: serde_json::json!({"state": "ok", "n": 2}),
            ts_ms: 9,
        };
        let back: CachedJsonSample =
            serde_json::from_str(&serde_json::to_string(&s).unwrap()).unwrap();
        assert_eq!(back, s);
    }

    #[test]
    fn key_shape_and_hostile_metric() {
        assert_eq!(
            last_cache_key(7, "cap-1", "soil_moisture"),
            "pnex:last:v1:7:cap-1:soil_moisture"
        );
        // Hostile chars are sanitized as an idempotent safety net (`:` is
        // part of the metric charset, see sanitize_metric_name).
        assert_eq!(
            last_cache_key(1, "dev", "a b:c!d"),
            "pnex:last:v1:1:dev:a_b:c_d"
        );
        // Idempotent: feeding an already-sanitized metric changes nothing.
        assert_eq!(
            last_cache_key(1, "dev", "soil_moisture"),
            last_cache_key(1, "dev", "soil_moisture")
        );
    }
}
