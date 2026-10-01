//! Process-wide shared Valkey connection for small cross-pod state
//! (device liveness leases, rate-limit counters, OAuth2 native bridge).
//! Established on first use from `settings.valkey.url`; `None` when Valkey
//! is not configured — callers then fall back to a local in-memory store
//! (device liveness refuses to boot instead, D108).
//!
//! One connection per tokio runtime: a `ConnectionManager` is driven by a
//! task of the runtime that created it and dies with it. A server has one
//! runtime (one connection); test binaries boot one runtime per test.

use std::sync::Mutex;

use loco_rs::config::Config;
use redis::aio::ConnectionManager;
use tokio::runtime::{Handle, Id};

use crate::services::last_cache::ValkeySettings;

/// Connections by (runtime, url); bounded — entries of finished runtimes
/// (tests) are evicted oldest first.
static CONNS: Mutex<Vec<(Id, String, ConnectionManager)>> = Mutex::new(Vec::new());
const MAX_CONNS: usize = 8;

/// Shared connection (cheap clone), `None` = Valkey not configured.
pub async fn conn(config: &Config) -> Option<ConnectionManager> {
    let url = ValkeySettings::from_config(config)?.url?;
    conn_url(&url).await
}

/// Shared connection to `url` for the current runtime.
pub async fn conn_url(url: &str) -> Option<ConnectionManager> {
    let rt = Handle::current().id();
    let cached = CONNS
        .lock()
        .expect("valkey conns")
        .iter()
        .find(|(id, u, _)| *id == rt && u == url)
        .map(|(_, _, c)| c.clone());
    if cached.is_some() {
        return cached;
    }
    let conn = crate::services::last_cache::connect_url(url).await?;
    let mut conns = CONNS.lock().expect("valkey conns");
    // A concurrent first use may have won meanwhile: keep its connection.
    if let Some((_, _, c)) = conns.iter().find(|(id, u, _)| *id == rt && u == url) {
        return Some(c.clone());
    }
    if conns.len() >= MAX_CONNS {
        conns.remove(0);
    }
    conns.push((rt, url.to_string(), conn.clone()));
    Some(conn)
}
