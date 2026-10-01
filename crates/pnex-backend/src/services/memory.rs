//! Org shared memory (Valkey) — read side for the dashboards.
//!
//! The flow `memory-write` node stores `{v, ts_ms}` JSON entries under
//! `pnex:mem:v1:{org}:{key}` (contract: `pnex_core::memory`). This service
//! lists the live keys of an org (picker) and resolves numeric values
//! (widgets). Degraded by design: Valkey disabled/unreachable = empty
//! listing / `available: false`, never a 500.

use std::time::Duration;

use loco_rs::config::Config;
use redis::aio::ConnectionManager;

use pnex_core::memory::{
    memory_cache_key, memory_numeric, memory_numeric_fields, MemoryEntry, MemoryKeyInfo, MemoryRef,
    MemoryValue, MemoryValuesResponse,
};

/// Hard bound of one listing / resolution.
const VALKEY_TIMEOUT: Duration = Duration::from_secs(2);
/// Max keys returned by one listing (picker, not an export).
pub const MEMORY_KEYS_CAP: usize = 500;

/// Shared connection, established on first use (`None` = feature off).
async fn conn(config: &Config) -> Option<ConnectionManager> {
    crate::services::shared_valkey::conn(config).await
}

fn decode(raw: Option<String>) -> Option<MemoryEntry> {
    serde_json::from_str(&raw?).ok()
}

/// Live keys of the org, sorted, with their numeric fields.
pub async fn list_keys(config: &Config, org_id: i64) -> Vec<MemoryKeyInfo> {
    match conn(config).await {
        Some(conn) => list_keys_with(conn, org_id).await,
        None => Vec::new(),
    }
}

/// [`list_keys`] over an explicit connection (test entry point).
pub async fn list_keys_with(mut conn: ConnectionManager, org_id: i64) -> Vec<MemoryKeyInfo> {
    let prefix = memory_cache_key(org_id, "");
    let op = async move {
        let mut cursor: u64 = 0;
        let mut keys: Vec<String> = Vec::new();
        loop {
            let (next, batch): (u64, Vec<String>) = redis::cmd("SCAN")
                .arg(cursor)
                .arg("MATCH")
                .arg(format!("{prefix}*"))
                .arg("COUNT")
                .arg(200)
                .query_async(&mut conn)
                .await?;
            keys.extend(batch);
            cursor = next;
            if cursor == 0 || keys.len() >= MEMORY_KEYS_CAP {
                break;
            }
        }
        keys.truncate(MEMORY_KEYS_CAP);
        if keys.is_empty() {
            return Ok::<_, redis::RedisError>(Vec::new());
        }
        let raw: Vec<Option<String>> = redis::AsyncCommands::mget(&mut conn, &keys).await?;
        let mut out: Vec<MemoryKeyInfo> = keys
            .iter()
            .zip(raw)
            .filter_map(|(k, raw)| {
                let entry = decode(raw)?;
                Some(MemoryKeyInfo {
                    key: k.strip_prefix(&prefix)?.to_string(),
                    ts_ms: entry.ts_ms,
                    fields: memory_numeric_fields(&entry.v),
                })
            })
            .collect();
        out.sort_by(|a, b| a.key.cmp(&b.key));
        Ok(out)
    };
    match tokio::time::timeout(VALKEY_TIMEOUT, op).await {
        Ok(Ok(keys)) => keys,
        Ok(Err(e)) => {
            tracing::warn!(error = %e, "memory key listing failed");
            Vec::new()
        }
        Err(_) => {
            tracing::warn!("memory key listing timed out");
            Vec::new()
        }
    }
}

/// Numeric values of `refs`, same order. Invalid refs and non-numeric
/// values resolve as unavailable items.
pub async fn values(config: &Config, org_id: i64, refs: &[MemoryRef]) -> MemoryValuesResponse {
    values_with(conn(config).await, org_id, refs).await
}

/// [`values`] over an explicit connection, `None` = feature off (test entry
/// point).
pub async fn values_with(
    conn: Option<ConnectionManager>,
    org_id: i64,
    refs: &[MemoryRef],
) -> MemoryValuesResponse {
    let unavailable = |available| MemoryValuesResponse {
        available,
        results: refs
            .iter()
            .map(|_| MemoryValue {
                available: false,
                value: None,
                ts_ms: None,
            })
            .collect(),
    };
    if refs.is_empty() {
        return unavailable(true);
    }
    let Some(mut conn) = conn else {
        return unavailable(false);
    };
    let keys: Vec<String> = refs
        .iter()
        .map(|r| memory_cache_key(org_id, &r.key))
        .collect();
    let op =
        async move { redis::AsyncCommands::mget::<_, Vec<Option<String>>>(&mut conn, keys).await };
    let raw = match tokio::time::timeout(VALKEY_TIMEOUT, op).await {
        Ok(Ok(raw)) => raw,
        Ok(Err(e)) => {
            tracing::warn!(error = %e, "memory values read failed");
            return unavailable(false);
        }
        Err(_) => {
            tracing::warn!("memory values read timed out");
            return unavailable(false);
        }
    };
    let results = refs
        .iter()
        .zip(raw)
        .map(|(r, raw)| {
            let entry = r.is_valid().then(|| decode(raw)).flatten();
            let value = entry.as_ref().and_then(|e| memory_numeric(&e.v, &r.field));
            MemoryValue {
                available: value.is_some(),
                value,
                ts_ms: value.and(entry.map(|e| e.ts_ms)),
            }
        })
        .collect();
    MemoryValuesResponse {
        available: true,
        results,
    }
}
