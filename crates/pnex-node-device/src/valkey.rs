//! Valkey live last-value cache reader for the `pnex-device-read` node.
//!
//! Reads `pnex:last:v1:{org}:{device}:{metric}` entries written by the
//! backend `ValkeyTapSink`. One `MGET` per execution (single round-trip),
//! freshness decided locally by `pnex_core::last_cache::resolve` against
//! `window_secs`: the freshness window becomes the exact age of the last
//! received sample, instead of the data age in OpenObserve (which inherits
//! the ingestion batcher latency). Any cache error degrades to the legacy
//! per-pin OpenObserve path - never blocks or crashes the flow.

use std::time::Duration;

use edgelink_core::{EdgelinkError, Result};
use redis::aio::ConnectionManager;

use pnex_core::CachedSample;

/// Valkey last-value cache client - one multiplexed connection shared by
/// every device-read node of the runtime process.
pub struct ValkeyLastClient {
    conn: ConnectionManager,
}

impl ValkeyLastClient {
    /// `Ok(None)` when `VALKEY_URL` is absent or blank (feature off - the
    /// legacy OpenObserve path). `Err` when set but unparseable - the node
    /// is rejected at build, school of `O2Client::from_env` (fail loud at
    /// deploy, visible cause). The connection is established lazily on the
    /// first command: `build()` is sync, deploy preflight must never block
    /// on a TCP connect.
    pub fn from_env_opt() -> Result<Option<Self>> {
        let url = match std::env::var("VALKEY_URL") {
            Ok(u) if !u.trim().is_empty() => u,
            _ => return Ok(None),
        };
        Self::open(&url).map(Some)
    }

    /// Opens a client from an explicit URL (test entry point).
    pub fn open(url: &str) -> Result<Self> {
        let client = redis::Client::open(url).map_err(|e| {
            EdgelinkError::InvalidOperation(format!("pnex-device-read : invalid VALKEY_URL: {e}"))
        })?;
        let conn = ConnectionManager::new_lazy_with_config(
            client,
            redis::aio::ConnectionManagerConfig::new(),
        )
        .map_err(|e| {
            EdgelinkError::InvalidOperation(format!(
                "pnex-device-read : cannot create valkey client: {e}"
            ))
        })?;
        Ok(Self { conn })
    }

    /// One MGET for all pins, hard-bounded at 1 s. Undecodable entries
    /// become `None` (a miss); only connection-level failures return `Err`
    /// (the caller falls back to the legacy OpenObserve path).
    pub async fn mget_last(
        &self,
        org_id: i64,
        device_id: &str,
        pins: &[String],
    ) -> std::result::Result<Vec<Option<CachedSample>>, String> {
        let keys: Vec<String> = pins
            .iter()
            .map(|p| {
                pnex_core::last_cache_key(
                    org_id,
                    device_id,
                    &pnex_core::normalize_measurement_name(p),
                )
            })
            .collect();
        let mut conn = self.conn.clone();
        let op = async move {
            let raw: Vec<Option<String>> = redis::AsyncCommands::mget(&mut conn, keys)
                .await
                .map_err(|e| format!("valkey mget failed: {e}"))?;
            let samples = raw
                .iter()
                .map(|s| {
                    s.as_deref()
                        .and_then(|s| serde_json::from_str::<CachedSample>(s).ok())
                })
                .collect();
            Ok(samples)
        };
        match tokio::time::timeout(Duration::from_secs(1), op).await {
            Ok(r) => r,
            Err(_) => Err("valkey mget timed out (1 s)".to_string()),
        }
    }
}
