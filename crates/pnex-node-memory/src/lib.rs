//! Org shared memory flow nodes (Valkey) — contract in [`pnex_core::memory`].
//!
//! - `pnex-memory-write`: stores `msg.payload` (any JSON) under a key with a
//!   lifetime (`SET … EX ttl`), then passes the message through unchanged.
//! - `pnex-memory-read`: on each incoming message, reads every configured
//!   key in one `MGET`; port 0 = object `{key: value|null}`, then one port
//!   per key (`payload` = value, `topic` = key; muted when missing/stale).
//!
//! Keys are scoped per organization (`pnex_org_id` stamped by the
//! projection): any flow of the org reads what another flow wrote, and the
//! dashboards read the same entries through the backend.

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use redis::aio::ConnectionManager;
use serde::Deserialize;
use tokio_util::sync::CancellationToken;

use edgelink_core::runtime::context::*;
use edgelink_core::runtime::flow::*;
use edgelink_core::runtime::model::json::*;
use edgelink_core::runtime::model::*;
use edgelink_core::runtime::nodes::*;
use edgelink_core::{EdgelinkError, Result};
use edgelink_macro::*;

use pnex_core::memory::{
    memory_cache_key, resolve_memory, MemoryEntry, MemoryReadConfig, MemoryWriteConfig,
    MEMORY_VALUE_MAX_BYTES,
};

/// Anti-stripping anchor: without a reference to the crate the linker may
/// drop the `inventory` submissions (same guard as the runtime binary calls).
pub fn registered() {}

/// Hard bound of one Valkey command.
const VALKEY_TIMEOUT: Duration = Duration::from_secs(1);

/// Lazily-connected Valkey client from `VALKEY_URL` (required: the memory
/// nodes have no fallback store). `build()` is sync — never connect here.
fn valkey_from_env(node: &str) -> Result<ConnectionManager> {
    let url = std::env::var("VALKEY_URL")
        .ok()
        .filter(|u| !u.trim().is_empty())
        .ok_or_else(|| {
            EdgelinkError::InvalidOperation(format!(
                "{node} [memory_store_unavailable] : VALKEY_URL is not set in the runtime environment"
            ))
        })?;
    let client = redis::Client::open(url.as_str()).map_err(|e| {
        EdgelinkError::InvalidOperation(format!("{node} : invalid VALKEY_URL: {e}"))
    })?;
    ConnectionManager::new_lazy_with_config(client, redis::aio::ConnectionManagerConfig::new())
        .map_err(|e| EdgelinkError::InvalidOperation(format!("{node} : valkey client: {e}")).into())
}

fn org_id_or_reject(node: &str, org_id: i64) -> Result<i64> {
    if org_id <= 0 {
        return Err(EdgelinkError::BadFlowsJson(format!(
            "{node} : pnex_org_id missing from the artifact (redeploy the flow)"
        ))
        .into());
    }
    Ok(org_id)
}

fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

fn to_variant(node: &str, v: serde_json::Value) -> Result<Variant> {
    serde_json::from_value(v).map_err(|e| {
        EdgelinkError::InvalidOperation(format!("{node} : non-convertible value : {e}")).into()
    })
}

// ───────────────────────────── memory-write ─────────────────────────────

const WRITE: &str = "pnex-memory-write";

#[derive(Debug, Deserialize)]
struct WriteNodeConfig {
    #[serde(flatten)]
    config: MemoryWriteConfig,
    #[serde(default)]
    pnex_org_id: i64,
}

#[flow_node("pnex-memory-write", red_name = "pnex-memory-write")]
struct MemoryWriteNode {
    base: BaseFlowNodeState,
    config: MemoryWriteConfig,
    cache_key: String,
    valkey: ConnectionManager,
}

impl MemoryWriteNode {
    fn build(
        _flow: &Flow,
        base_node: BaseFlowNodeState,
        config: &RedFlowNodeConfig,
        _options: Option<&config::Config>,
    ) -> Result<Box<dyn FlowNodeBehavior>> {
        let cfg = WriteNodeConfig::deserialize(&config.rest)
            .map_err(|e| EdgelinkError::BadFlowsJson(format!("{WRITE} : invalid config : {e}")))?;
        if let Some((code, message)) = cfg.config.check() {
            return Err(
                EdgelinkError::BadFlowsJson(format!("{WRITE} [{code}] : {message}")).into(),
            );
        }
        let org_id = org_id_or_reject(WRITE, cfg.pnex_org_id)?;
        Ok(Box::new(MemoryWriteNode {
            base: base_node,
            cache_key: memory_cache_key(org_id, &cfg.config.key),
            config: cfg.config,
            valkey: valkey_from_env(WRITE)?,
        }))
    }

    async fn execute(&self, msg: MsgHandle, cancel: CancellationToken) -> Result<()> {
        let payload = {
            let m = msg.read().await;
            match m.get("payload") {
                Some(v) => serde_json::to_value(v).map_err(|e| {
                    EdgelinkError::InvalidOperation(format!(
                        "{WRITE} : non-serializable payload : {e}"
                    ))
                })?,
                None => serde_json::Value::Null,
            }
        };
        let raw = serde_json::to_string(&MemoryEntry {
            v: payload,
            ts_ms: now_ms(),
        })
        .map_err(|e| EdgelinkError::InvalidOperation(format!("{WRITE} : encode: {e}")))?;
        if raw.len() > MEMORY_VALUE_MAX_BYTES {
            return Err(EdgelinkError::InvalidOperation(format!(
                "{WRITE} [memory_value_too_large] : {} bytes (max {MEMORY_VALUE_MAX_BYTES})",
                raw.len()
            ))
            .into());
        }
        let mut conn = self.valkey.clone();
        let key = self.cache_key.clone();
        let ttl = u64::from(self.config.ttl_secs);
        let op =
            async move { redis::AsyncCommands::set_ex::<_, _, ()>(&mut conn, key, raw, ttl).await };
        match tokio::time::timeout(VALKEY_TIMEOUT, op).await {
            Ok(Ok(())) => {}
            Ok(Err(e)) => {
                return Err(EdgelinkError::InvalidOperation(format!(
                    "{WRITE} : valkey write failed for \"{}\": {e}",
                    self.config.key
                ))
                .into())
            }
            Err(_) => {
                return Err(EdgelinkError::InvalidOperation(format!(
                    "{WRITE} : valkey write timed out (1 s) for \"{}\"",
                    self.config.key
                ))
                .into())
            }
        }
        self.fan_out_one(Envelope { port: 0, msg }, cancel).await
    }
}

#[async_trait]
impl FlowNodeBehavior for MemoryWriteNode {
    fn get_base(&self) -> &BaseFlowNodeState {
        &self.base
    }

    async fn run(self: Arc<Self>, stop_token: CancellationToken) {
        while !stop_token.is_cancelled() {
            let cancel = stop_token.child_token();
            with_uow(
                self.as_ref(),
                cancel.child_token(),
                |node: &MemoryWriteNode, msg: MsgHandle| async move {
                    match node.execute(msg.clone(), cancel.child_token()).await {
                        Ok(()) => Ok(()),
                        Err(e) => {
                            log::warn!("{WRITE} [{}] : message rejected : {e}", node.name());
                            Err(e)
                        }
                    }
                },
            )
            .await;
        }
    }
}

// ───────────────────────────── memory-read ──────────────────────────────

const READ: &str = "pnex-memory-read";

#[derive(Debug, Deserialize)]
struct ReadNodeConfig {
    #[serde(flatten)]
    config: MemoryReadConfig,
    #[serde(default)]
    pnex_org_id: i64,
}

#[flow_node("pnex-memory-read", red_name = "pnex-memory-read")]
struct MemoryReadNode {
    base: BaseFlowNodeState,
    config: MemoryReadConfig,
    cache_keys: Vec<String>,
    valkey: ConnectionManager,
}

impl MemoryReadNode {
    fn build(
        _flow: &Flow,
        base_node: BaseFlowNodeState,
        config: &RedFlowNodeConfig,
        _options: Option<&config::Config>,
    ) -> Result<Box<dyn FlowNodeBehavior>> {
        let cfg = ReadNodeConfig::deserialize(&config.rest)
            .map_err(|e| EdgelinkError::BadFlowsJson(format!("{READ} : invalid config : {e}")))?;
        if let Some((code, message)) = cfg.config.check() {
            return Err(EdgelinkError::BadFlowsJson(format!("{READ} [{code}] : {message}")).into());
        }
        let org_id = org_id_or_reject(READ, cfg.pnex_org_id)?;
        Ok(Box::new(MemoryReadNode {
            base: base_node,
            cache_keys: cfg
                .config
                .keys
                .iter()
                .map(|k| memory_cache_key(org_id, k))
                .collect(),
            config: cfg.config,
            valkey: valkey_from_env(READ)?,
        }))
    }

    /// One MGET for every key; undecodable entries read as missing.
    async fn fetch(&self) -> Result<Vec<Option<MemoryEntry>>> {
        let mut conn = self.valkey.clone();
        let keys = self.cache_keys.clone();
        let op = async move {
            redis::AsyncCommands::mget::<_, Vec<Option<String>>>(&mut conn, keys).await
        };
        let raw = match tokio::time::timeout(VALKEY_TIMEOUT, op).await {
            Ok(Ok(raw)) => raw,
            Ok(Err(e)) => {
                return Err(EdgelinkError::InvalidOperation(format!(
                    "{READ} : valkey read failed: {e}"
                ))
                .into())
            }
            Err(_) => {
                return Err(EdgelinkError::InvalidOperation(format!(
                    "{READ} : valkey read timed out (1 s)"
                ))
                .into())
            }
        };
        Ok(raw
            .into_iter()
            .map(|s| s.and_then(|s| serde_json::from_str::<MemoryEntry>(&s).ok()))
            .collect())
    }

    async fn execute(&self, msg: MsgHandle, cancel: CancellationToken) -> Result<()> {
        // A single-key MGET still returns a list (redis-rs normalizes it).
        let entries = self.fetch().await?;
        let now = now_ms();
        let mut obj = serde_json::Map::new();
        let mut scalars = Vec::with_capacity(entries.len());
        for (key, entry) in self.config.keys.iter().zip(entries) {
            let value = resolve_memory(entry, now, self.config.max_age_secs).map(|e| e.v);
            if value.is_none() {
                log::debug!("{READ} [{}] : \"{key}\" missing or stale", self.name());
            }
            obj.insert(
                key.clone(),
                value.clone().unwrap_or(serde_json::Value::Null),
            );
            scalars.push(value);
        }

        let port_count = self.get_base().ports.len();
        let mut envelopes: smallvec::SmallVec<[Envelope; 4]> = smallvec::SmallVec::new();
        {
            let mut m = msg.write().await;
            m.set(
                "payload".to_string(),
                to_variant(READ, serde_json::Value::Object(obj))?,
            );
        }
        envelopes.push(Envelope { port: 0, msg });
        for (i, (key, value)) in self.config.keys.iter().zip(scalars).enumerate() {
            let port = 1 + i;
            let Some(value) = value else { continue };
            if port >= port_count {
                continue;
            }
            let mut m = Msg::default();
            m.set("payload".to_string(), to_variant(READ, value)?);
            m.set("topic".to_string(), Variant::String(key.clone()));
            envelopes.push(Envelope {
                port,
                msg: MsgHandle::new(m),
            });
        }
        self.fan_out_many(envelopes, cancel).await
    }
}

#[async_trait]
impl FlowNodeBehavior for MemoryReadNode {
    fn get_base(&self) -> &BaseFlowNodeState {
        &self.base
    }

    async fn run(self: Arc<Self>, stop_token: CancellationToken) {
        while !stop_token.is_cancelled() {
            let cancel = stop_token.child_token();
            with_uow(
                self.as_ref(),
                cancel.child_token(),
                |node: &MemoryReadNode, msg: MsgHandle| async move {
                    match node.execute(msg.clone(), cancel.child_token()).await {
                        Ok(()) => Ok(()),
                        Err(e) => {
                            log::warn!("{READ} [{}] : message rejected : {e}", node.name());
                            Err(e)
                        }
                    }
                },
            )
            .await;
        }
    }
}
