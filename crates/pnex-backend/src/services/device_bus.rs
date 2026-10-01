//! Cross-pod device command bus (D107) over Valkey.
//!
//! A device WebSocket lives on ONE pod; a command for it may originate on
//! any pod (a flow worker, an API call, a regulator cast, an OTA push).
//! Routing is pod-addressed, not device-addressed, so every pod keeps a
//! single subscription whatever the number of devices:
//! - route: `pnex:devroute:v1:{device_registry_id}` = `<pod id>|<session>`,
//!   `EX 90`, set when the session opens, refreshed every 30 s by the
//!   session, deleted on close by THAT session only (a quick reconnect to
//!   the same pod is never erased by the late cleanup of the old one);
//! - channel: `pnex:devcmd:v1:{pod id}`, payload `{"d": id, "m": json}`
//!   where `m` is the already serialized `ServerMsg` (the receiving pod
//!   forwards it verbatim to the local session);
//! - `PUBLISH` returns the number of receivers: 0 = the routed pod is gone
//!   (stale route removed, the device counts as offline).
//!
//! Without Valkey the bus is off and commands reach local sessions only
//! (single-node deployments — unchanged behavior).

use std::sync::OnceLock;
use std::time::Duration;

use futures_util::StreamExt;
use loco_rs::app::AppContext;
use redis::aio::ConnectionManager;
use redis::AsyncCommands;
use serde::{Deserialize, Serialize};

/// Route TTL; the session refreshes it every [`ROUTE_REFRESH`].
const ROUTE_TTL_SECS: u64 = 90;
pub const ROUTE_REFRESH: Duration = Duration::from_secs(30);

struct Bus {
    conn: ConnectionManager,
    pod: String,
}

/// One bus per tokio runtime (one per process on a server; per test in the
/// test binaries, see `runtime_local`).
static BUS: crate::services::runtime_local::PerRuntime<std::sync::Arc<Bus>> =
    crate::services::runtime_local::PerRuntime::new();

#[derive(Serialize, Deserialize)]
struct Envelope {
    /// Target device registry id.
    d: i64,
    /// Serialized `ServerMsg`.
    m: String,
}

pub fn route_key(device_registry_id: i64) -> String {
    format!("pnex:devroute:v1:{device_registry_id}")
}

pub fn pod_channel(pod: &str) -> String {
    format!("pnex:devcmd:v1:{pod}")
}

/// Identity of this process on the bus (host + pid + start time: unique per
/// process start, like a flow worker boot).
pub fn pod_id() -> &'static str {
    static ID: OnceLock<String> = OnceLock::new();
    ID.get_or_init(|| {
        let host = std::env::var("HOSTNAME")
            .ok()
            .filter(|h| !h.trim().is_empty())
            .or_else(|| std::fs::read_to_string("/proc/sys/kernel/hostname").ok())
            .map(|h| h.trim().to_string())
            .unwrap_or_else(|| "pod".into());
        let ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis())
            .unwrap_or(0);
        format!("{host}:{}:{ms}", std::process::id())
    })
}

/// Starts the bus (no-op without Valkey): a command connection plus the
/// subscription of this pod's channel, forwarding to local sessions.
pub async fn spawn(ctx: &AppContext) {
    let Some(settings) = crate::services::last_cache::ValkeySettings::from_config(&ctx.config)
    else {
        tracing::info!("device bus: valkey not configured — commands reach local sessions only");
        return;
    };
    let Some(url) = settings.url.filter(|u| !u.trim().is_empty()) else {
        return;
    };
    let Some(conn) = crate::services::last_cache::connect_opt(&ctx.config).await else {
        return;
    };
    start(conn, url, pod_id().to_string(), |d, m| {
        crate::controllers::ws_device::push_local_json(d, m)
    });
}

/// Starts the bus on an explicit connection / pod id / local sink
/// (public for tests: two "pods" in one process).
pub fn start(
    conn: ConnectionManager,
    url: String,
    pod: String,
    sink: impl Fn(i64, String) -> bool + Send + Sync + 'static,
) {
    if !BUS.set(std::sync::Arc::new(Bus {
        conn,
        pod: pod.clone(),
    })) {
        return; // already running (tests booting several apps)
    }
    tokio::spawn(subscribe_loop(url, pod, sink));
}

/// Subscribes `pod_channel(pod)` and forwards each envelope to `sink`,
/// reconnecting with a bounded backoff.
pub async fn subscribe_loop(
    url: String,
    pod: String,
    sink: impl Fn(i64, String) -> bool + Send + Sync + 'static,
) {
    let channel = pod_channel(&pod);
    let mut backoff = Duration::from_millis(500);
    loop {
        match redis::Client::open(url.as_str()) {
            Ok(client) => match client.get_async_pubsub().await {
                Ok(mut pubsub) => {
                    if let Err(e) = pubsub.subscribe(&channel).await {
                        tracing::warn!(error = %e, "device bus: subscribe failed");
                    } else {
                        tracing::info!(%channel, "device bus subscribed");
                        backoff = Duration::from_millis(500);
                        let mut stream = pubsub.on_message();
                        while let Some(msg) = stream.next().await {
                            let Ok(payload) = msg.get_payload::<String>() else {
                                continue;
                            };
                            match serde_json::from_str::<Envelope>(&payload) {
                                Ok(env) => {
                                    if !sink(env.d, env.m) {
                                        tracing::debug!(
                                            device = env.d,
                                            "device bus: no local session (moved or closed)"
                                        );
                                    }
                                }
                                Err(e) => tracing::warn!(error = %e, "device bus: bad envelope"),
                            }
                        }
                        tracing::warn!("device bus: subscription closed — reconnecting");
                    }
                }
                Err(e) => tracing::warn!(error = %e, "device bus: pubsub connection failed"),
            },
            Err(e) => {
                tracing::error!(error = %e, "device bus: invalid valkey url");
                return;
            }
        }
        tokio::time::sleep(backoff).await;
        backoff = (backoff * 2).min(Duration::from_secs(10));
    }
}

fn bus() -> Option<std::sync::Arc<Bus>> {
    BUS.get()
}

fn route_value(pod: &str, session: &str) -> String {
    format!("{pod}|{session}")
}

/// Pod part of a route value.
fn pod_of(route: &str) -> &str {
    route.split_once('|').map_or(route, |(pod, _)| pod)
}

/// The device session `session` opened on this pod (or refreshes it).
pub async fn claim(device_registry_id: i64, session: &str) {
    let Some(bus) = bus() else {
        return;
    };
    let mut conn = bus.conn.clone();
    let res: redis::RedisResult<()> = conn
        .set_ex(
            route_key(device_registry_id),
            route_value(&bus.pod, session),
            ROUTE_TTL_SECS,
        )
        .await;
    if let Err(e) = res {
        tracing::warn!(device = device_registry_id, error = %e, "device bus: route claim failed");
    }
}

/// The session `session` closed on this pod: drop the route if it is
/// still that session's.
pub async fn release(device_registry_id: i64, session: &str) {
    let Some(bus) = bus() else {
        return;
    };
    let mut conn = bus.conn.clone();
    let script = redis::Script::new(
        "if redis.call('GET', KEYS[1]) == ARGV[1] then return redis.call('DEL', KEYS[1]) else return 0 end",
    );
    let res: redis::RedisResult<i64> = script
        .key(route_key(device_registry_id))
        .arg(route_value(&bus.pod, session))
        .invoke_async(&mut conn)
        .await;
    if let Err(e) = res {
        tracing::warn!(device = device_registry_id, error = %e, "device bus: route release failed");
    }
}

/// `true` when the bus runs (Valkey configured): the route is then the
/// authority on where the live session of a device is.
pub fn enabled() -> bool {
    BUS.get().is_some()
}

/// Route refresh by the session holding it: rewritten only while it is
/// still this session's (or expired) — a superseded session never steals
/// the route back from the newer one.
pub async fn refresh(device_registry_id: i64, session: &str) {
    let Some(bus) = bus() else {
        return;
    };
    let mut conn = bus.conn.clone();
    let res: redis::RedisResult<i64> = redis::Script::new(
        "local cur = redis.call('GET', KEYS[1]) \
         if cur == false or cur == ARGV[1] then \
           redis.call('SET', KEYS[1], ARGV[1], 'EX', ARGV[2]) return 1 \
         end return 0",
    )
    .key(route_key(device_registry_id))
    .arg(route_value(&bus.pod, session))
    .arg(ROUTE_TTL_SECS)
    .invoke_async(&mut conn)
    .await;
    match res {
        Ok(0) => tracing::debug!(
            device = device_registry_id,
            "device bus: route held by a newer session"
        ),
        Ok(_) => {}
        Err(e) => {
            tracing::warn!(device = device_registry_id, error = %e, "device bus: route refresh failed")
        }
    }
}

/// Where the live session of a device is, according to the bus.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Route {
    /// Bus off (no Valkey): only local sessions exist.
    Off,
    /// Valkey unreachable: unknown (callers fall back to local sessions).
    Unknown,
    /// No route: the device is offline on every pod.
    Missing,
    /// Raw route value `<pod>|<session>`.
    At(String),
}

impl Route {
    /// `true` when the route designates `session` on this pod.
    pub fn is_local_session(&self, session: &str) -> bool {
        match (self, bus()) {
            (Route::At(raw), Some(bus)) => *raw == route_value(&bus.pod, session),
            _ => false,
        }
    }
}

/// Looks up the route of a device (one `GET`).
pub async fn lookup(device_registry_id: i64) -> Route {
    let Some(bus) = bus() else {
        return Route::Off;
    };
    let mut conn = bus.conn.clone();
    match tokio::time::timeout(
        Duration::from_secs(2),
        conn.get::<_, Option<String>>(route_key(device_registry_id)),
    )
    .await
    {
        Ok(Ok(Some(raw))) => Route::At(raw),
        Ok(Ok(None)) => Route::Missing,
        Ok(Err(e)) => {
            tracing::warn!(device = device_registry_id, error = %e, "device bus: route lookup failed");
            Route::Unknown
        }
        Err(_) => {
            tracing::warn!(
                device = device_registry_id,
                "device bus: route lookup timed out"
            );
            Route::Unknown
        }
    }
}

/// Raw route value (`<pod>|<session>`) of the device.
async fn route_raw(device_registry_id: i64) -> Option<String> {
    match lookup(device_registry_id).await {
        Route::At(raw) => Some(raw),
        _ => None,
    }
}

/// Pod currently holding the session of the device (`None`: offline or
/// bus off).
pub async fn route_of(device_registry_id: i64) -> Option<String> {
    route_raw(device_registry_id)
        .await
        .map(|r| pod_of(&r).to_string())
}

/// Sends a serialized `ServerMsg` to the pod holding the device session.
/// `false` = no route, or the routed pod is gone (route removed).
pub async fn send(device_registry_id: i64, json: String) -> bool {
    let Some(raw) = route_raw(device_registry_id).await else {
        return false;
    };
    send_via(device_registry_id, &raw, json).await
}

/// Publishes a serialized `ServerMsg` to the pod of an already looked-up
/// route (`raw` = `<pod>|<session>`). A route nobody listens on is stale:
/// it is removed (if unchanged) and `false` is returned.
pub async fn send_via(device_registry_id: i64, raw: &str, json: String) -> bool {
    let Some(bus) = bus() else {
        return false;
    };
    let pod = pod_of(raw).to_string();
    let payload = match serde_json::to_string(&Envelope {
        d: device_registry_id,
        m: json,
    }) {
        Ok(p) => p,
        Err(_) => return false,
    };
    let mut conn = bus.conn.clone();
    match conn.publish::<_, _, i64>(pod_channel(&pod), payload).await {
        Ok(n) if n > 0 => true,
        Ok(_) => {
            // Nobody listens on that pod any more: the route is stale.
            let _: redis::RedisResult<i64> = redis::Script::new(
                "if redis.call('GET', KEYS[1]) == ARGV[1] then return redis.call('DEL', KEYS[1]) else return 0 end",
            )
            .key(route_key(device_registry_id))
            .arg(raw)
            .invoke_async(&mut conn)
            .await;
            false
        }
        Err(e) => {
            tracing::warn!(device = device_registry_id, error = %e, "device bus: publish failed");
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    /// Two pods in one process against a real Valkey
    /// (`PNEX_TEST_VALKEY_URL`, skipped otherwise): route, publish, stale
    /// route cleanup, owner-only release, and `push_command` falling back
    /// to the bus for a device whose session lives on another pod.
    #[tokio::test]
    async fn commands_reach_the_pod_holding_the_session() {
        let Ok(url) = std::env::var("PNEX_TEST_VALKEY_URL") else {
            eprintln!("PNEX_TEST_VALKEY_URL unset — device bus test skipped");
            return;
        };
        let suffix = std::process::id();
        let api_pod = format!("test-api-{suffix}");
        let dev_pod = format!("test-dev-{suffix}");
        let client = redis::Client::open(url.as_str()).unwrap();
        let conn = ConnectionManager::new(client).await.unwrap();
        start(conn.clone(), url.clone(), api_pod.clone(), |_, _| false);

        // The "device pod" subscribes its own channel and records.
        let got: Arc<Mutex<Vec<(i64, String)>>> = Arc::default();
        let sink = got.clone();
        tokio::spawn(subscribe_loop(url.clone(), dev_pod.clone(), move |d, m| {
            sink.lock().unwrap().push((d, m));
            true
        }));
        tokio::time::sleep(Duration::from_millis(300)).await;

        let device = 990_000_000 + i64::from(suffix % 1000);
        let mut c = conn.clone();
        let _: () = c
            .set_ex(route_key(device), route_value(&dev_pod, "s1"), 30)
            .await
            .unwrap();
        assert_eq!(route_of(device).await.as_deref(), Some(dev_pod.as_str()));

        // push_command: no local session here → relayed to the device pod.
        let msg = pnex_core::ServerMsg::Reject {
            reason: "bus-test".into(),
        };
        assert!(crate::controllers::ws_device::push_command(device, msg.clone()).await);
        assert!(crate::controllers::ws_device::is_connected(device).await);
        for _ in 0..50 {
            if !got.lock().unwrap().is_empty() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        let received = got.lock().unwrap().clone();
        assert_eq!(received.len(), 1, "{received:?}");
        assert_eq!(received[0].0, device);
        assert_eq!(
            received[0].1,
            serde_json::to_string(&msg).unwrap(),
            "verbatim JSON"
        );

        // A route to a pod nobody listens on is stale: offline + removed.
        let gone = device + 1;
        let _: () = c
            .set_ex(route_key(gone), "pod-that-died", 30)
            .await
            .unwrap();
        assert!(!send(gone, "{}".into()).await);
        assert_eq!(route_of(gone).await, None, "stale route removed");
        assert!(!crate::controllers::ws_device::is_connected(gone).await);

        // Owner-only release: a quick reconnect (new session, same pod) is
        // never erased by the late cleanup of the previous session.
        let mine = device + 2;
        claim(mine, "old").await;
        claim(mine, "new").await;
        release(mine, "old").await;
        assert_eq!(
            route_of(mine).await.as_deref(),
            Some(api_pod.as_str()),
            "reconnect kept"
        );
        release(mine, "new").await;
        assert_eq!(route_of(mine).await, None);
        release(device, "s1").await; // held by the device pod: untouched
        assert_eq!(route_of(device).await.as_deref(), Some(dev_pod.as_str()));
        let _: () = c.del(route_key(device)).await.unwrap();

        // Route authority: a stale local session (half-open, awaiting its
        // watchdog) never wins over the route to the pod holding the live
        // session; the local fast path is taken only when the route
        // designates the local session itself.
        let stale = device + 3;
        let mut local_rx = crate::controllers::ws_device::register_local_for_test(stale, "stale");
        let _: () = c
            .set_ex(route_key(stale), route_value(&dev_pod, "live"), 30)
            .await
            .unwrap();
        got.lock().unwrap().clear();
        assert!(crate::controllers::ws_device::push_command(stale, msg.clone()).await);
        for _ in 0..50 {
            if !got.lock().unwrap().is_empty() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        assert_eq!(got.lock().unwrap().len(), 1, "relayed to the live pod");
        assert_eq!(got.lock().unwrap()[0].0, stale);
        assert!(local_rx.try_recv().is_err(), "stale local session bypassed");

        let _: () = c
            .set_ex(route_key(stale), route_value(&api_pod, "stale"), 30)
            .await
            .unwrap();
        got.lock().unwrap().clear();
        assert!(crate::controllers::ws_device::push_command(stale, msg.clone()).await);
        assert!(matches!(
            local_rx.try_recv(),
            Ok(crate::controllers::ws_device::Downlink::Msg(_))
        ));
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert!(got.lock().unwrap().is_empty(), "local fast path, no relay");
        drop(local_rx);
        let _: () = c.del(route_key(stale)).await.unwrap();

        // Refresh never steals a route held by a newer session.
        let moved = device + 4;
        claim(moved, "old").await;
        let _: () = c
            .set_ex(route_key(moved), route_value(&dev_pod, "new"), 30)
            .await
            .unwrap();
        refresh(moved, "old").await;
        assert_eq!(route_of(moved).await.as_deref(), Some(dev_pod.as_str()));
        let _: () = c.del(route_key(moved)).await.unwrap();
        refresh(moved, "old").await; // expired route: re-taken
        assert_eq!(route_of(moved).await.as_deref(), Some(api_pod.as_str()));
        let _: () = c.del(route_key(moved)).await.unwrap();
    }
}
