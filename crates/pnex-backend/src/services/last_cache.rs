//! Live last-value telemetry cache (Valkey) — write side.
//!
//! Mirrors every numeric telemetry point into Valkey at WS reception so the
//! flow-runtime `pnex-device-read` node can serve freshness windows far below
//! the OpenObserve ingestion batcher latency (~10 s default). School of the
//! O2 batcher / `GpsTapSink`: **never block** the WS loop — the cache write
//! runs off-thread, errors are logged and swallowed. Valkey is an
//! accelerator: OpenObserve remains the storage of record, losing the cache
//! never loses data.

use std::sync::Arc;

use loco_rs::config::Config;
use redis::aio::ConnectionManager;

use crate::services::telemetry::{TelemetryPoint, TelemetrySink};

/// `settings.valkey` — absent or blank `url` = feature off (legacy O2-only
/// path). Tolerant of absent sections, same school as IngestSettings
/// (partial yaml must never fail config parsing).
#[derive(Clone, Debug)]
pub struct ValkeySettings {
    pub url: Option<String>,
}

impl ValkeySettings {
    /// Reads `settings.valkey.url` — blank counts as absent (feature off).
    pub fn from_config(config: &Config) -> Option<Self> {
        let url = config
            .settings
            .as_ref()?
            .get("valkey")?
            .get("url")?
            .as_str()?
            .trim()
            .to_string();
        (!url.is_empty()).then_some(Self { url: Some(url) })
    }
}

/// Connects to Valkey when `settings.valkey.url` is set. Eager connect at
/// boot; on failure, degrades to a lazy manager that self-heals when Valkey
/// comes up (the compose container may race the backend boot). `None` =
/// feature off.
pub async fn connect_opt(config: &Config) -> Option<ConnectionManager> {
    let settings = ValkeySettings::from_config(config)?;
    connect_url(settings.url.as_deref().unwrap_or_default()).await
}

/// [`connect_opt`] from an explicit url (eager, lazy fallback).
pub async fn connect_url(url: &str) -> Option<ConnectionManager> {
    let client = match redis::Client::open(url) {
        Ok(c) => c,
        Err(e) => {
            tracing::warn!(error = %e, "invalid valkey url — live cache disabled");
            return None;
        }
    };
    match ConnectionManager::new(client).await {
        Ok(conn) => Some(conn),
        Err(e) => {
            // Valkey not up yet (or unreachable): keep going with the lazy
            // manager — it retries on every command until it succeeds.
            tracing::warn!(error = %e, "valkey unreachable at boot — lazy reconnect enabled");
            Some(
                ConnectionManager::new_lazy_with_config(
                    redis::Client::open(url).ok()?,
                    redis::aio::ConnectionManagerConfig::new(),
                )
                .ok()?,
            )
        }
    }
}

/// Builds the cache entry of a telemetry point — `None` for non-numeric
/// values (identical filter to the O2 remote-write, `promwrite.rs`: the
/// cache never contains a series OpenObserve does not have).
pub fn cached_sample_of(point: &TelemetryPoint) -> Option<(String, pnex_core::CachedSample)> {
    let v: f64 = point.value.trim().parse().ok()?;
    let key = pnex_core::last_cache_key(point.org_id, &point.device_id, &point.metric_name);
    Some((
        key,
        pnex_core::CachedSample {
            v,
            ts_ms: point.timestamp.timestamp_millis(),
        },
    ))
}

/// Tap sink mirroring numeric points into Valkey. Wrap position: between the
/// GPS tap (outermost) and the O2 channel sink (innermost). The forward to
/// the inner sink happens first and unconditionally (O2 latency unaffected),
/// then the cache write runs off-thread (fire-and-forget).
pub struct ValkeyTapSink {
    conn: ConnectionManager,
    inner: Arc<dyn TelemetrySink>,
}

impl ValkeyTapSink {
    pub fn new(conn: ConnectionManager, inner: Arc<dyn TelemetrySink>) -> Arc<Self> {
        Arc::new(Self { conn, inner })
    }
}

impl TelemetrySink for ValkeyTapSink {
    fn send(&self, point: TelemetryPoint) {
        // Inner first, always — the pipeline latency stays untouched (the
        // inner O2 sink itself drops live-only points, `record == false`).
        let ttl = ttl_of(&point);
        let sample = cached_sample_of(&point);
        self.inner.send(point);
        let Some((key, sample)) = sample else {
            return;
        };
        let conn = self.conn.clone();
        tokio::spawn(async move {
            let payload = match serde_json::to_string(&sample) {
                Ok(p) => p,
                Err(e) => {
                    tracing::warn!(error = %e, "valkey last-cache payload encode failed");
                    return;
                }
            };
            // Drop, not block: cache is best-effort.
            if let Err(e) = set_if_newer(conn, &key, &payload, sample.ts_ms, ttl).await {
                tracing::warn!(error = %e, "valkey last-cache write failed");
            }
        });
    }
}

/// Cache TTL of a point: live-only points (no OpenObserve fallback, D95)
/// keep their last value a week; recorded points keep the historic GC TTL.
pub fn ttl_of(point: &TelemetryPoint) -> i64 {
    if point.record {
        pnex_core::LAST_SAMPLE_TTL_SECS
    } else {
        pnex_core::AGENT_LIVE_ONLY_TTL_SECS
    }
}

/// Atomic "SET EX unless a newer sample is already cached": a replayed
/// point from an edge agent disk buffer (D95) must never overwrite a fresher
/// live value. Entries are `{"v":…, "ts_ms":…}` JSON (numeric and JSON
/// samples alike); an undecodable current entry is overwritten.
const SET_IF_NEWER_LUA: &str = r#"
local cur = redis.call('GET', KEYS[1])
if cur then
  local ok, d = pcall(cjson.decode, cur)
  if ok and type(d) == 'table' and tonumber(d.ts_ms) and tonumber(d.ts_ms) > tonumber(ARGV[2]) then
    return 0
  end
end
redis.call('SET', KEYS[1], ARGV[1], 'EX', ARGV[3])
return 1
"#;

/// Runs [`SET_IF_NEWER_LUA`] — `Ok(true)` when written.
pub async fn set_if_newer(
    mut conn: ConnectionManager,
    key: &str,
    payload: &str,
    ts_ms: i64,
    ttl_secs: i64,
) -> redis::RedisResult<bool> {
    let written: i64 = redis::Script::new(SET_IF_NEWER_LUA)
        .key(key)
        .arg(payload)
        .arg(ts_ms)
        .arg(ttl_secs)
        .invoke_async(&mut conn)
        .await?;
    Ok(written == 1)
}

// ─────────────── GPIO last values of generic devices (pins UI) ───────────────
//
// `/ws/device` StateReports feed the "last value" column of the Pins panel.
// The session lives on one pod while `GET /pins` may hit any pod: values are
// mirrored into one Valkey hash per device (`field = gpio`, `value = JSON`),
// written by a single background writer fed by a bounded channel (never
// blocks the WS loop, drops under pressure), cleared when the session that
// owns the lease closes. Without Valkey the per-pod map of `ws_device`
// remains the only source (single-node).

use std::collections::HashMap;

use tokio::sync::mpsc;

/// TTL of a device GPIO hash, refreshed on every write: covers the slowest
/// sane subscribe period; a crashed pod's leftovers expire on their own.
pub const GPIO_TTL_SECS: i64 = 86_400;
/// Bound of the GPIO writer queue (points dropped beyond, logged).
const GPIO_QUEUE: usize = 4096;

/// Hash of the GPIO last values of one device.
pub fn gpio_key(device_registry_id: i64) -> String {
    format!("pnex:gpio:v1:{device_registry_id}")
}

/// One write of the GPIO writer.
#[derive(Debug)]
pub enum GpioOp {
    /// Last value of one gpio (`value` = serialized JSON).
    Put {
        device_registry_id: i64,
        gpio: i32,
        value: String,
    },
    /// Session closed: forget every value of the device.
    Clear { device_registry_id: i64 },
}

static GPIO_TX: crate::services::runtime_local::PerRuntime<Option<mpsc::Sender<GpioOp>>> =
    crate::services::runtime_local::PerRuntime::new();

/// Process-wide Valkey connection (`None` = feature off), established once.
pub async fn shared(config: &Config) -> Option<ConnectionManager> {
    crate::services::shared_valkey::conn(config).await
}

/// Applies one GPIO op on `conn` (public for tests).
pub async fn apply_gpio_op(conn: &mut ConnectionManager, op: GpioOp) -> redis::RedisResult<()> {
    match op {
        GpioOp::Put {
            device_registry_id,
            gpio,
            value,
        } => {
            let key = gpio_key(device_registry_id);
            redis::pipe()
                .atomic()
                .hset(&key, gpio, value)
                .ignore()
                .expire(&key, GPIO_TTL_SECS)
                .ignore()
                .query_async(conn)
                .await
        }
        GpioOp::Clear { device_registry_id } => {
            redis::cmd("DEL")
                .arg(gpio_key(device_registry_id))
                .query_async(conn)
                .await
        }
    }
}

/// Queue of the single GPIO writer task (`None` = Valkey off).
async fn gpio_queue(config: &Config) -> Option<mpsc::Sender<GpioOp>> {
    GPIO_TX
        .get_or_init(|| async {
            let mut conn = shared(config).await?;
            let (tx, mut rx) = mpsc::channel::<GpioOp>(GPIO_QUEUE);
            tokio::spawn(async move {
                while let Some(op) = rx.recv().await {
                    if let Err(e) = apply_gpio_op(&mut conn, op).await {
                        tracing::warn!(error = %e, "valkey gpio last-value write failed");
                    }
                }
            });
            Some(tx)
        })
        .await
}

/// Enqueues a GPIO op (no-op without Valkey; dropped when the queue is full).
pub async fn gpio_send(config: &Config, op: GpioOp) {
    if let Some(tx) = gpio_queue(config).await {
        if let Err(e) = tx.try_send(op) {
            tracing::debug!(error = %e, "gpio last-value queue full — dropped");
        }
    }
}

/// GPIO last values of a device on `conn` (public for tests).
pub async fn gpio_values_with(
    conn: &mut ConnectionManager,
    device_registry_id: i64,
) -> redis::RedisResult<HashMap<i32, serde_json::Value>> {
    let raw: HashMap<i32, String> = redis::cmd("HGETALL")
        .arg(gpio_key(device_registry_id))
        .query_async(conn)
        .await?;
    Ok(raw
        .into_iter()
        .filter_map(|(g, v)| serde_json::from_str(&v).ok().map(|v| (g, v)))
        .collect())
}

/// GPIO last values of a device, whatever pod holds its session. `None` =
/// Valkey off or unreachable (callers fall back to the local map).
pub async fn gpio_values(
    config: &Config,
    device_registry_id: i64,
) -> Option<HashMap<i32, serde_json::Value>> {
    let mut conn = shared(config).await?;
    match tokio::time::timeout(
        std::time::Duration::from_secs(2),
        gpio_values_with(&mut conn, device_registry_id),
    )
    .await
    {
        Ok(Ok(values)) => Some(values),
        Ok(Err(e)) => {
            tracing::warn!(error = %e, "valkey gpio last-value read failed");
            None
        }
        Err(_) => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn point(value: &str) -> TelemetryPoint {
        TelemetryPoint {
            org_id: 7,
            device_registry_id: 1,
            device_id: "cap-1".into(),
            pred_dev: "soil".into(),
            metric_name: "soil_moisture".into(),
            value: value.into(),
            timestamp: chrono::Utc::now(),
            ts_source: "server",
            source_type: "test",
            record: true,
        }
    }

    #[test]
    fn numeric_point_builds_key_and_sample() {
        let (key, sample) = cached_sample_of(&point("42.5")).expect("numeric value must cache");
        assert_eq!(key, "pnex:last:v1:7:cap-1:soil_moisture");
        assert_eq!(sample.v, 42.5);
        assert!(sample.ts_ms > 0);
    }

    #[test]
    fn non_numeric_point_is_skipped() {
        assert!(cached_sample_of(&point("n/a")).is_none());
        assert!(cached_sample_of(&point("")).is_none());
    }

    #[test]
    fn boolean_style_coercions_and_negative() {
        let (_, s1) = cached_sample_of(&point("0")).unwrap();
        assert_eq!(s1.v, 0.0);
        let (_, s2) = cached_sample_of(&point("-3.25")).unwrap();
        assert_eq!(s2.v, -3.25);
    }
}
