//! Livraison des notifications websocket (D53, amendé 2026-09-14 : pas de
//! centre de notif dans le front — le bus WS est un canal configurable
//! consommé par des clients externes).
//!
//! - broker en-process : `NOTIFY_SESSIONS` (mpsc par session, école
//!   `DEVICE_SESSIONS` de `ws_device` — pas de broadcast) ;
//! - [`deliver_in_app`] is the single websocket path (publish + journal):
//!   the internal endpoint (`/internal/notify/deliver`, flow node sends)
//!   and the Test button loop through it;
//! - the journal lives in OpenObserve (D86, `services::notify_journal`),
//!   retention is O2's — no table, no pruner;
//! - cross-pod fan-out: a browser `/ws/notify` session lives on ONE pod
//!   while the producer (internal deliver endpoint, OTA outcome, Test
//!   button) may run on any other. Every frame is delivered to the local
//!   sessions, then published once on the Valkey channel [`BUS_CHANNEL`];
//!   each pod subscribes it and delivers frames from other pods to its own
//!   sessions. Pods also exchange their per-org session counts (presence,
//!   refreshed every [`PRESENCE_REFRESH`]) so the `delivered` count stays a
//!   meaningful cluster-wide figure. Without Valkey the bus is off and
//!   frames reach local sessions only (single-node deployments).

use std::collections::HashMap;
use std::sync::{LazyLock, Mutex};
use std::time::{Duration, Instant};

use chrono::Utc;
use futures_util::StreamExt;
use loco_rs::prelude::*;
use redis::aio::ConnectionManager;
use redis::AsyncCommands;
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;
use uuid::Uuid;

use crate::services::settings::NotifySettings;

/// Sessions WS ouvertes dans CE process : org_id → (session → downlink).
/// Les frames sont publiées à toutes les sessions de l'org ; les sessions
/// fermées (receiver drop) sont élaguées au passage.
type SessionMap = HashMap<i64, HashMap<Uuid, mpsc::UnboundedSender<String>>>;
static NOTIFY_SESSIONS: LazyLock<Mutex<SessionMap>> = LazyLock::new(|| Mutex::new(HashMap::new()));

/// Enregistre une session (posé AVANT le spawn du loop).
pub(crate) fn register_session(org_id: i64, session: Uuid, tx: mpsc::UnboundedSender<String>) {
    NOTIFY_SESSIONS
        .lock()
        .expect("sessions notify")
        .entry(org_id)
        .or_default()
        .insert(session, tx);
    announce_presence();
}

/// Retire une session (RAII `SessionGuard` — tous chemins de sortie).
pub(crate) fn unregister_session(org_id: i64, session: Uuid) {
    NOTIFY_SESSIONS
        .lock()
        .expect("sessions notify")
        .entry(org_id)
        .or_default()
        .remove(&session);
    announce_presence();
}

/// Retire le device des registres à la sortie, tous chemins compris
/// (école `DeviceSessionGuard` de ws_device).
pub(crate) struct SessionGuard {
    org_id: i64,
    session: Uuid,
}

impl SessionGuard {
    pub(crate) fn new(org_id: i64, session: Uuid) -> Self {
        Self { org_id, session }
    }
}

impl Drop for SessionGuard {
    fn drop(&mut self) {
        unregister_session(self.org_id, self.session);
    }
}

/// Publishes a text frame to every session of the org, on every pod.
/// Returns the number of sessions reached: local sessions that accepted
/// the frame plus the sessions other pods last reported for the org
/// (presence ≤ [`PRESENCE_TTL`] old). The remote part is a best-effort
/// figure — the frame is fanned out through Valkey pub/sub, which does not
/// acknowledge per session. 0 = nobody connected anywhere: the frame is
/// lost, persistence goes through the journal (at-least-once is the
/// webhook semantics, not the live bus's).
pub fn publish(org_id: i64, frame: String) -> usize {
    let Some(bus) = BUS.get() else {
        return deliver_local(org_id, &frame);
    };
    let local = deliver_local(org_id, &frame);
    bus.send(&Envelope {
        p: bus.pod.clone(),
        o: Some(org_id),
        f: Some(frame),
        ..Default::default()
    });
    local + remote_sessions(org_id)
}

/// Delivers a frame to the sessions of the org open on THIS pod (closed
/// sessions are pruned on the way). Returns the number reached.
fn deliver_local(org_id: i64, frame: &str) -> usize {
    let mut sessions = NOTIFY_SESSIONS.lock().expect("sessions notify");
    let Some(org_sessions) = sessions.get_mut(&org_id) else {
        return 0;
    };
    org_sessions.retain(|_, tx| !tx.is_closed());
    let mut delivered = 0;
    for tx in org_sessions.values() {
        if tx.send(frame.to_string()).is_ok() {
            delivered += 1;
        }
    }
    delivered
}

// ───────────────────────── Cross-pod fan-out ─────────────────────────

/// Valkey pub/sub channel shared by every pod (frames + presence).
pub const BUS_CHANNEL: &str = "pnex:notify:v1";
/// Presence older than this is ignored (a pod that died stops counting).
pub const PRESENCE_TTL: Duration = Duration::from_secs(90);
/// Presence heartbeat period.
pub const PRESENCE_REFRESH: Duration = Duration::from_secs(30);

/// Bus message. Plain struct with optional fields (no internally tagged
/// enum: serde buffering breaks under `arbitrary_precision`).
#[derive(Debug, Default, Clone, Serialize, Deserialize, PartialEq)]
pub struct Envelope {
    /// Sender pod id (a pod ignores its own messages).
    pub p: String,
    /// Frame: target org.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub o: Option<i64>,
    /// Frame: serialized `NotifyItem`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub f: Option<String>,
    /// Presence: full snapshot of the sender's `(org, sessions)` counts.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub c: Option<Vec<(i64, u32)>>,
    /// Presence request: every pod answers with its snapshot (sent by a
    /// pod that just (re)subscribed).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub q: bool,
}

struct Bus {
    pod: String,
    /// Ordered queue drained by one publisher task (frames keep their
    /// order, the caller never waits on Valkey).
    tx: mpsc::UnboundedSender<String>,
}

impl Bus {
    fn send(&self, env: &Envelope) {
        if let Ok(payload) = serde_json::to_string(env) {
            let _ = self.tx.send(payload);
        }
    }
}

/// One bus per tokio runtime (see `runtime_local`).
static BUS: crate::services::runtime_local::PerRuntime<std::sync::Arc<Bus>> =
    crate::services::runtime_local::PerRuntime::new();
/// Session counts reported by the other pods: pod → (received, org → n).
type RemoteMap = HashMap<String, (Instant, HashMap<i64, u32>)>;
static REMOTE: LazyLock<Mutex<RemoteMap>> = LazyLock::new(|| Mutex::new(HashMap::new()));

/// Starts the cross-pod fan-out (no-op without Valkey).
pub async fn spawn_bus(ctx: &AppContext) {
    let Some(url) = crate::services::last_cache::ValkeySettings::from_config(&ctx.config)
        .and_then(|s| s.url)
        .filter(|u| !u.trim().is_empty())
    else {
        tracing::info!("notify bus: valkey not configured — frames reach local sessions only");
        return;
    };
    let Some(conn) = crate::services::last_cache::connect_opt(&ctx.config).await else {
        return;
    };
    start(conn, url, crate::services::device_bus::pod_id().to_string());
}

/// Starts the bus on an explicit connection / pod id (public for tests:
/// several "pods" in one process).
pub fn start(conn: ConnectionManager, url: String, pod: String) {
    let (tx, rx) = mpsc::unbounded_channel::<String>();
    if !BUS.set(std::sync::Arc::new(Bus {
        pod: pod.clone(),
        tx,
    })) {
        return; // already running (tests booting several apps)
    }
    tokio::spawn(publisher_loop(conn, rx));
    let me = pod.clone();
    tokio::spawn(subscribe_loop(
        url,
        |_| {
            // (Re)subscribed: ask the others for their presence and give ours.
            if let Some(bus) = BUS.get() {
                bus.send(&Envelope {
                    p: bus.pod.clone(),
                    q: true,
                    ..Default::default()
                });
            }
            announce_presence();
        },
        move |env| handle(&me, env),
    ));
    tokio::spawn(async {
        let mut tick = tokio::time::interval(PRESENCE_REFRESH);
        loop {
            tick.tick().await;
            announce_presence();
        }
    });
}

async fn publisher_loop(mut conn: ConnectionManager, mut rx: mpsc::UnboundedReceiver<String>) {
    while let Some(payload) = rx.recv().await {
        let res: redis::RedisResult<i64> = conn.publish(BUS_CHANNEL, payload).await;
        if let Err(e) = res {
            tracing::warn!(error = %e, "notify bus: publish failed (frame reached local sessions only)");
        }
    }
}

/// Subscribes [`BUS_CHANNEL`] and hands every envelope to `on_message`,
/// reconnecting with a bounded backoff; `on_subscribed` runs after each
/// (re)subscription. Public for tests (a second "pod").
pub async fn subscribe_loop(
    url: String,
    on_subscribed: impl Fn(()) + Send + Sync + 'static,
    on_message: impl Fn(Envelope) + Send + Sync + 'static,
) {
    let mut backoff = Duration::from_millis(500);
    loop {
        match redis::Client::open(url.as_str()) {
            Ok(client) => match client.get_async_pubsub().await {
                Ok(mut pubsub) => {
                    if let Err(e) = pubsub.subscribe(BUS_CHANNEL).await {
                        tracing::warn!(error = %e, "notify bus: subscribe failed");
                    } else {
                        backoff = Duration::from_millis(500);
                        on_subscribed(());
                        let mut stream = pubsub.on_message();
                        while let Some(msg) = stream.next().await {
                            let Ok(payload) = msg.get_payload::<String>() else {
                                continue;
                            };
                            match serde_json::from_str::<Envelope>(&payload) {
                                Ok(env) => on_message(env),
                                Err(e) => tracing::warn!(error = %e, "notify bus: bad envelope"),
                            }
                        }
                        tracing::warn!("notify bus: subscription closed — reconnecting");
                    }
                }
                Err(e) => tracing::warn!(error = %e, "notify bus: pubsub connection failed"),
            },
            Err(e) => {
                tracing::error!(error = %e, "notify bus: invalid valkey url");
                return;
            }
        }
        tokio::time::sleep(backoff).await;
        backoff = (backoff * 2).min(Duration::from_secs(10));
    }
}

/// Handles one envelope received by the pod `me`.
fn handle(me: &str, env: Envelope) {
    if env.p == me {
        return;
    }
    if let (Some(org_id), Some(frame)) = (env.o, env.f.as_deref()) {
        deliver_local(org_id, frame);
    }
    if let Some(counts) = env.c {
        REMOTE.lock().expect("notify presence").insert(
            env.p.clone(),
            (Instant::now(), counts.into_iter().collect()),
        );
    }
    if env.q {
        announce_presence();
    }
}

/// Live session counts of this pod, per org (closed sessions skipped).
fn local_counts() -> Vec<(i64, u32)> {
    NOTIFY_SESSIONS
        .lock()
        .expect("sessions notify")
        .iter()
        .map(|(org, s)| (*org, s.values().filter(|tx| !tx.is_closed()).count() as u32))
        .filter(|(_, n)| *n > 0)
        .collect()
}

/// Publishes this pod's presence snapshot (no-op without the bus).
fn announce_presence() {
    let Some(bus) = BUS.get() else {
        return;
    };
    bus.send(&Envelope {
        p: bus.pod.clone(),
        c: Some(local_counts()),
        ..Default::default()
    });
}

/// Sessions of the org other pods reported recently.
fn remote_sessions(org_id: i64) -> usize {
    let mut remote = REMOTE.lock().expect("notify presence");
    remote.retain(|_, (at, _)| at.elapsed() < PRESENCE_TTL);
    remote
        .values()
        .filter_map(|(_, counts)| counts.get(&org_id))
        .map(|n| *n as usize)
        .sum()
}

/// Publishes the WS frame to the org bus and journals the attempt (D86,
/// O2) — the single websocket delivery path (internal deliver endpoint,
/// Test loopback, OTA outcomes). `entry` carries the context (channel,
/// source, flow); status, subject and reached sessions are filled here
/// (`delivered` = local sessions + sessions other pods reported, see
/// [`publish`] — a best-effort cluster-wide count).
pub fn deliver_in_app(
    mut entry: pnex_core::NotifyDeliveryEntry,
    msg: &pnex_notify::Message,
) -> usize {
    let now = Utc::now();
    // The frame id is the journal timestamp (µs) — no relational row.
    let frame = serde_json::to_string(&pnex_core::NotifyItem {
        id: now.timestamp_micros(),
        subject: msg.subject.clone(),
        body: msg.body.clone(),
        meta: msg.meta.clone(),
        created_at: now.to_rfc3339(),
    })
    .unwrap_or_default();
    let delivered = publish(entry.org_id, frame);
    tracing::debug!(
        "deliver_in_app: org={} channel={} source={} sessions={delivered}",
        entry.org_id,
        entry.channel_id,
        entry.source
    );
    entry.channel_kind = "websocket".into();
    entry.status = pnex_core::DELIVERY_SENT.into();
    entry.subject = msg.subject.clone();
    entry.delivered = Some(delivered as i64);
    crate::services::notify_journal::submit(entry);
    delivered
}

/// Config runtime d'un canal websocket — composée serveur (URL de deliver
/// + token interne), injectée au nœud flow par env (`apply_runtime_env`)
///   et utilisée en loopback par le bouton Test. Jamais en DB ni flows.json.
pub fn in_app_runtime_config(
    settings: &NotifySettings,
    org_id: i64,
    channel_id: Uuid,
) -> serde_json::Value {
    serde_json::json!({
        "deliver_url": settings.deliver_url,
        "token": settings.internal_token.clone().unwrap_or_default(),
        "org_id": org_id,
        "channel_id": channel_id.to_string(),
    })
}

/// Boot warning: without the internal token the websocket channel cannot
/// deliver from flows.
pub fn warn_missing_token(ctx: &AppContext) {
    if NotifySettings::from_config(&ctx.config)
        .internal_token
        .is_none()
    {
        tracing::warn!(
            "notifications : internal_token absent (PNEX_NOTIFY_INTERNAL_TOKEN) — \
             /internal/notify/deliver rejettera toute livraison websocket"
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn publish_sans_session_renvoie_zero() {
        assert_eq!(publish(987_654_321, "{}".into()), 0);
    }

    #[tokio::test]
    async fn session_enregistree_recoit_puis_elaguee() {
        let (tx, mut rx) = mpsc::unbounded_channel::<String>();
        register_session(555, Uuid::nil(), tx);
        assert_eq!(publish(555, "frame".into()), 1);
        assert_eq!(rx.recv().await.as_deref(), Some("frame"));
        unregister_session(555, Uuid::nil());
        assert_eq!(publish(555, "frame2".into()), 0);
    }

    /// Two pods in one process against a real Valkey
    /// (`PNEX_TEST_VALKEY_URL`, skipped otherwise): a frame published here
    /// reaches the other pod, a frame from the other pod reaches the local
    /// sessions, presence is exchanged and counted in `delivered`.
    #[tokio::test]
    async fn frames_and_presence_cross_pods() {
        let Ok(url) = std::env::var("PNEX_TEST_VALKEY_URL") else {
            eprintln!("PNEX_TEST_VALKEY_URL unset — notify bus test skipped");
            return;
        };
        let suffix = std::process::id();
        let pod_a = format!("test-notify-a-{suffix}");
        let pod_b = format!("test-notify-b-{suffix}");
        let org = 880_000_000 + i64::from(suffix % 100_000);
        let client = redis::Client::open(url.as_str()).unwrap();
        let conn = ConnectionManager::new(client).await.unwrap();
        start(conn.clone(), url.clone(), pod_a.clone());

        // Pod B records what it receives (and ignores its own messages).
        let got: std::sync::Arc<Mutex<Vec<Envelope>>> = Default::default();
        let sink = got.clone();
        let me_b = pod_b.clone();
        tokio::spawn(subscribe_loop(
            url.clone(),
            |_| {},
            move |env| {
                if env.p != me_b {
                    sink.lock().unwrap().push(env);
                }
            },
        ));
        tokio::time::sleep(Duration::from_millis(400)).await;

        let (tx, mut rx) = mpsc::unbounded_channel::<String>();
        let session = Uuid::new_v4();
        register_session(org, session, tx);
        assert_eq!(publish(org, "from-a".into()), 1);
        assert_eq!(rx.recv().await.as_deref(), Some("from-a"));

        let mut c = conn.clone();
        let wait_for = |pred: Box<dyn Fn(&Envelope) -> bool>| {
            let got = got.clone();
            async move {
                for _ in 0..100 {
                    if got.lock().unwrap().iter().any(|e| pred(e)) {
                        return true;
                    }
                    tokio::time::sleep(Duration::from_millis(20)).await;
                }
                false
            }
        };
        assert!(
            wait_for(Box::new(
                move |e| e.o == Some(org) && e.f.as_deref() == Some("from-a")
            ))
            .await,
            "frame fanned out to the other pod"
        );
        assert!(
            wait_for(Box::new(move |e| e
                .c
                .as_ref()
                .is_some_and(|c| c.contains(&(org, 1)))))
            .await,
            "presence announced on register"
        );

        // A frame produced on pod B reaches the session held here.
        let remote = Envelope {
            p: pod_b.clone(),
            o: Some(org),
            f: Some("from-b".into()),
            ..Default::default()
        };
        let _: i64 = c
            .publish(BUS_CHANNEL, serde_json::to_string(&remote).unwrap())
            .await
            .unwrap();
        let frame = tokio::time::timeout(Duration::from_secs(3), rx.recv())
            .await
            .unwrap();
        assert_eq!(frame.as_deref(), Some("from-b"));

        // Pod B reports 2 sessions for the org: counted in `delivered`.
        let presence = Envelope {
            p: pod_b.clone(),
            c: Some(vec![(org, 2)]),
            ..Default::default()
        };
        let _: i64 = c
            .publish(BUS_CHANNEL, serde_json::to_string(&presence).unwrap())
            .await
            .unwrap();
        let mut delivered = 0;
        for _ in 0..100 {
            delivered = publish(org, "count".into());
            if delivered == 3 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        assert_eq!(delivered, 3, "1 local + 2 reported by the other pod");

        // A presence request is answered with this pod's snapshot.
        got.lock().unwrap().clear();
        let ask = Envelope {
            p: pod_b.clone(),
            q: true,
            ..Default::default()
        };
        let _: i64 = c
            .publish(BUS_CHANNEL, serde_json::to_string(&ask).unwrap())
            .await
            .unwrap();
        assert!(
            wait_for(Box::new(move |e| e
                .c
                .as_ref()
                .is_some_and(|c| c.contains(&(org, 1)))))
            .await,
            "presence request answered"
        );
        unregister_session(org, session);
    }
}
