//! CameraHub (camera-video.md D74/D76) — in-memory live fan-out of camera
//! frames plus the Valkey frame bus towards the flow runtime.
//!
//! - One [`Entry`] per camera device: last frame, `broadcast` channel for
//!   live viewers (capacity 4 — slow viewers lose frames, the ingest never
//!   waits), viewer count, uplink state.
//! - Frames are also published on Valkey (`SET frame EX 15` + `PUBLISH
//!   meta`) through a bounded queue drained by one task: when Valkey is slow
//!   or down, frames are dropped for the flows, never for the live view.
//! - Capture demand (D76): `continuous` settings, or at least one live
//!   viewer, enable the device uplink through `ServerMsg::CameraConfig`; the
//!   last viewer leaving disables it after [`ON_DEMAND_GRACE_SECS`].
//! - Flow demand (D102): a `camera-source` node of a deployed flow keeps
//!   its camera awake exactly like a viewer, so `on_demand` only governs the
//!   browser live view.
//! - Cluster (several API pods, no sticky sessions): the uplink, the live
//!   viewers and the flow workers may sit on different pods. With Valkey:
//!   - a viewer whose camera uplink is not local is fed by a per-pod
//!     relay of the frame bus (one subscription per camera per pod, shared
//!     by the local viewers, dropped with the last one);
//!   - viewer counts and flow demand are cluster-wide (hashes
//!     [`VIEWERS_KEY`] / [`FLOW_KEY`], one field per pod / flow worker with
//!     an expiry refreshed by a per-pod heartbeat; expired fields of a dead
//!     pod are pruned and the camera config re-pushed);
//!   - the uplink holds a cross-pod claim ([`uplink_key`], owner-checked)
//!     that also tells every pod whether the camera is streaming;
//!   - the latest frame metadata ([`latest_key`]) serves snapshots and
//!     `last_frame_ms` from any pod.
//!   Without Valkey everything stays process-local (single-node).

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, LazyLock, Mutex, OnceLock};
use std::time::Duration;

use futures_util::StreamExt;
use redis::AsyncCommands;

use axum::body::Bytes;
use loco_rs::prelude::*;
use pnex_core::camera::{BusFrameMeta, CameraSettings, CaptureMode, FrameHeader, FrameSize};
use pnex_core::ServerMsg;
use redis::aio::ConnectionManager;
use sea_orm::{ActiveValue::Set, ColumnTrait, EntityTrait, QueryFilter};
use serde::{Deserialize, Serialize};
use tokio::sync::{broadcast, mpsc};

use crate::controllers::ws_device;
use crate::models::_entities::{device_cameras, device_registries};

/// Delay between the last live viewer leaving and the uplink stop
/// (`on_demand` mode) — absorbs page reloads and tab switches.
pub const ON_DEMAND_GRACE_SECS: u64 = 30;
/// Live broadcast capacity per device.
const LIVE_CAPACITY: usize = 4;
/// Pending frames towards Valkey (all devices) before dropping.
const BUS_QUEUE: usize = 64;

/// One received frame.
#[derive(Debug)]
pub struct Frame {
    pub org_id: i64,
    pub device: i64,
    /// Device slug (Valkey bus keys).
    pub device_id: Arc<str>,
    pub header: FrameHeader,
    /// Server reception time, unix ms.
    pub ts_ms: i64,
    pub jpeg: Bytes,
}

struct Entry {
    org_id: i64,
    tx: broadcast::Sender<Arc<Frame>>,
    latest: Option<Arc<Frame>>,
    viewers: u32,
    uplink: bool,
    /// Minimum spacing between accepted frames (server-side fps guard).
    min_interval_ms: i64,
    last_accepted_ms: i64,
    dropped: u64,
    /// Bumped on every viewer join — a pending grace-stop checks it is
    /// still the latest generation before disabling the uplink.
    generation: u64,
    /// A bus relay feeds this entry (frames of a remote uplink).
    relay: bool,
}

impl Entry {
    fn new(org_id: i64) -> Self {
        let (tx, _) = broadcast::channel(LIVE_CAPACITY);
        Self {
            org_id,
            tx,
            latest: None,
            viewers: 0,
            uplink: false,
            min_interval_ms: 0,
            last_accepted_ms: 0,
            dropped: 0,
            generation: 0,
            relay: false,
        }
    }
}

static HUB: LazyLock<Mutex<HashMap<i64, Entry>>> = LazyLock::new(|| Mutex::new(HashMap::new()));
static BUS: crate::services::runtime_local::PerRuntime<mpsc::Sender<Arc<Frame>>> =
    crate::services::runtime_local::PerRuntime::new();
/// Cameras wanted by deployed flows (D102), keyed like the flow artifact:
/// `(org_id, device slug)`, per source flow worker (D106: every local
/// worker publishes the demand of its own artifact).
static FLOW_DEMAND: LazyLock<Mutex<HashMap<String, HashSet<(i64, String)>>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));
/// Database handle for demand transitions triggered outside a request
/// (flow deploys, boot).
static DB: OnceLock<DatabaseConnection> = OnceLock::new();

fn with_entry<R>(device: i64, org_id: i64, f: impl FnOnce(&mut Entry) -> R) -> R {
    let mut hub = HUB.lock().expect("camera hub");
    let entry = hub.entry(device).or_insert_with(|| Entry::new(org_id));
    entry.org_id = org_id;
    f(entry)
}

/// Starts the Valkey frame publisher (no-op without Valkey: live works,
/// flow camera nodes report `camera_bus_unavailable`).
pub async fn spawn_bus(ctx: &AppContext) {
    let _ = DB.set(ctx.db.clone());
    let Some(conn) = crate::services::last_cache::connect_opt(&ctx.config).await else {
        tracing::info!("camera bus: valkey not configured — flow camera nodes disabled");
        return;
    };
    let url = crate::services::last_cache::ValkeySettings::from_config(&ctx.config)
        .and_then(|s| s.url)
        .unwrap_or_default();
    start_cluster(conn, url, crate::services::device_bus::pod_id().to_string());
}

/// Starts the frame publisher and the cluster state on an explicit
/// connection / pod id (public for tests).
pub fn start_cluster(conn: ConnectionManager, url: String, pod: String) {
    let (tx, rx) = mpsc::channel::<Arc<Frame>>(BUS_QUEUE);
    if !BUS.set(tx) {
        return; // already running (tests booting several apps)
    }
    let _ = CLUSTER.set(Arc::new(Cluster {
        conn: conn.clone(),
        url,
        pod,
    }));
    tokio::spawn(bus_loop(conn, rx));
    tokio::spawn(heartbeat_loop());
}

async fn bus_loop(mut conn: ConnectionManager, mut rx: mpsc::Receiver<Arc<Frame>>) {
    while let Some(frame) = rx.recv().await {
        let key = pnex_core::camera::frame_key(frame.org_id, &frame.device_id, frame.header.seq);
        let meta = BusFrameMeta {
            seq: frame.header.seq,
            ts_ms: frame.ts_ms,
            width: frame.header.width,
            height: frame.header.height,
            size: frame.jpeg.len() as u32,
            key: key.clone(),
        };
        let Ok(meta) = serde_json::to_string(&meta) else {
            continue;
        };
        let channel = pnex_core::camera::bus_channel(frame.org_id, &frame.device_id);
        let res: redis::RedisResult<()> = redis::pipe()
            .set_ex(&key, frame.jpeg.as_ref(), pnex_core::camera::FRAME_TTL_SECS)
            .ignore()
            .set_ex(
                latest_key(frame.org_id, &frame.device_id),
                &meta,
                LATEST_TTL_SECS,
            )
            .ignore()
            .publish(&channel, meta)
            .ignore()
            .query_async(&mut conn)
            .await;
        if let Err(e) = res {
            tracing::warn!(error = %e, "camera bus publish failed");
        }
    }
}

/// Accepts one decrypted frame: fps guard, live fan-out, bus publish.
/// Returns `false` when the frame was dropped by the fps guard.
pub fn publish(
    org_id: i64,
    device: i64,
    device_id: &Arc<str>,
    header: FrameHeader,
    jpeg: Bytes,
) -> bool {
    let ts_ms = chrono::Utc::now().timestamp_millis();
    let frame = with_entry(device, org_id, |e| {
        if e.min_interval_ms > 0 && ts_ms - e.last_accepted_ms < e.min_interval_ms {
            e.dropped += 1;
            return None;
        }
        e.last_accepted_ms = ts_ms;
        let frame = Arc::new(Frame {
            org_id,
            device,
            device_id: device_id.clone(),
            header,
            ts_ms,
            jpeg,
        });
        e.latest = Some(frame.clone());
        // No receiver = no viewer: the error is expected.
        let _ = e.tx.send(frame.clone());
        Some(frame)
    });
    let Some(frame) = frame else {
        return false;
    };
    if let Some(bus) = BUS.get() {
        // Full queue = Valkey lagging: drop for flows, never block the ingest.
        let _ = bus.try_send(frame);
    }
    true
}

/// Marks the `/ws/camera` uplink open/closed.
pub fn set_uplink(org_id: i64, device: i64, open: bool) {
    with_entry(device, org_id, |e| e.uplink = open);
}

/// Live subscription: latest frame (sent first) + receiver.
pub fn subscribe(
    org_id: i64,
    device: i64,
) -> (Option<Arc<Frame>>, broadcast::Receiver<Arc<Frame>>) {
    with_entry(device, org_id, |e| (e.latest.clone(), e.tx.subscribe()))
}

/// Last frame received for a device by THIS pod (uplink or relay).
pub fn latest(device: i64) -> Option<Arc<Frame>> {
    HUB.lock()
        .expect("camera hub")
        .get(&device)
        .and_then(|e| e.latest.clone())
}

/// Last frame of a device from any pod (snapshot, live detect fallback):
/// the local one, else the frame referenced by the cluster latest-frame
/// metadata (gone [`pnex_core::camera::FRAME_TTL_SECS`] after the last
/// frame — the bus never keeps old JPEG bytes).
pub async fn latest_anywhere(org_id: i64, device: i64, device_slug: &str) -> Option<Arc<Frame>> {
    if let Some(frame) = latest(device) {
        return Some(frame);
    }
    let cluster = CLUSTER.get()?;
    let mut conn = cluster.conn.clone();
    let raw: Option<String> = conn
        .get(latest_key(org_id, device_slug))
        .await
        .ok()
        .flatten();
    let meta: BusFrameMeta = serde_json::from_str(&raw?).ok()?;
    let jpeg: Option<Vec<u8>> = conn.get(&meta.key).await.ok().flatten();
    Some(Arc::new(frame_of_meta(
        org_id,
        device,
        device_slug,
        &meta,
        jpeg?,
    )))
}

fn frame_of_meta(
    org_id: i64,
    device: i64,
    slug: &str,
    meta: &BusFrameMeta,
    jpeg: Vec<u8>,
) -> Frame {
    Frame {
        org_id,
        device,
        device_id: Arc::from(slug),
        header: FrameHeader {
            seq: meta.seq,
            uptime_ms: 0,
            width: meta.width,
            height: meta.height,
        },
        ts_ms: meta.ts_ms,
        jpeg: Bytes::from(jpeg),
    }
}

/// Drops this pod's in-memory state of a camera (last frame, counters).
/// Test helper: database ids are recycled between integration tests while
/// the hub lives for the whole test process.
#[doc(hidden)]
pub fn forget_local(device: i64) {
    HUB.lock().expect("camera hub").remove(&device);
}

/// Live state for the camera list.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct LiveState {
    pub uplink: bool,
    pub viewers: u32,
    pub last_frame_ms: Option<i64>,
}

/// Live state as seen by THIS pod only.
pub fn live_state(device: i64) -> LiveState {
    let hub = HUB.lock().expect("camera hub");
    match hub.get(&device) {
        Some(e) => LiveState {
            uplink: e.uplink,
            viewers: e.viewers,
            last_frame_ms: e.latest.as_ref().map(|f| f.ts_ms),
        },
        None => LiveState::default(),
    }
}

/// Cluster-wide live state of several cameras `(device pk, org, slug)`:
/// uplink claimed on any pod, viewers of every pod, last frame seen by the
/// bus — merged with the local state (Valkey down or absent = local only).
pub async fn live_states(cameras: &[(i64, i64, &str)]) -> HashMap<i64, LiveState> {
    let mut out: HashMap<i64, LiveState> = cameras
        .iter()
        .map(|(d, _, _)| (*d, live_state(*d)))
        .collect();
    let Some(cluster) = CLUSTER.get() else {
        return out;
    };
    if cameras.is_empty() {
        return out;
    }
    let mut pipe = redis::pipe();
    for (device, org, slug) in cameras {
        pipe.exists(uplink_key(*device)).get(latest_key(*org, slug));
    }
    pipe.hgetall(VIEWERS_KEY);
    let mut conn = cluster.conn.clone();
    let res: redis::RedisResult<Vec<redis::Value>> = pipe.query_async(&mut conn).await;
    let values = match res {
        Ok(v) => v,
        Err(e) => {
            tracing::warn!(error = %e, "camera cluster: live state read failed (local view only)");
            return out;
        }
    };
    let mut it = values.into_iter();
    for (device, _, _) in cameras {
        let exists: bool = it
            .next()
            .and_then(|v| redis::from_redis_value(v).ok())
            .unwrap_or(false);
        let latest: Option<String> = it
            .next()
            .and_then(|v| redis::from_redis_value(v).ok())
            .flatten();
        let st = out.entry(*device).or_default();
        st.uplink |= exists;
        if let Some(meta) = latest.and_then(|m| serde_json::from_str::<BusFrameMeta>(&m).ok()) {
            st.last_frame_ms = Some(st.last_frame_ms.map_or(meta.ts_ms, |l| l.max(meta.ts_ms)));
        }
    }
    let viewers: HashMap<String, String> = it
        .next()
        .and_then(|v| redis::from_redis_value(v).ok())
        .unwrap_or_default();
    let now = now_ms();
    for (field, value) in &viewers {
        let Some((device, pod)) = parse_viewer_field(field) else {
            continue;
        };
        if pod == cluster.pod {
            continue; // the local count is already in
        }
        if let (Some(st), Some((n, exp))) = (out.get_mut(&device), parse_viewer_value(value)) {
            if exp > now {
                st.viewers += n;
            }
        }
    }
    out
}

// ───────────────────────────── Settings ─────────────────────────────

/// Settings DTO of a `device_cameras` row (unknown codes fall back to the
/// defaults — the row is written through the validated API only).
pub fn settings_of(row: &device_cameras::Model) -> CameraSettings {
    let d = CameraSettings::default();
    CameraSettings {
        framesize: FrameSize::from_wire(&row.framesize).unwrap_or(d.framesize),
        quality: u8::try_from(row.quality).unwrap_or(d.quality),
        fps: u8::try_from(row.fps).unwrap_or(d.fps),
        capture_mode: CaptureMode::from_wire(&row.capture_mode).unwrap_or(d.capture_mode),
        vflip: row.vflip,
        hmirror: row.hmirror,
    }
}

/// Creates the settings row with defaults when absent (announce of a
/// `camera` cap). Returns the current row.
pub async fn ensure_row(
    db: &DatabaseConnection,
    device: &device_registries::Model,
) -> Result<device_cameras::Model> {
    if let Some(row) = device_cameras::Entity::find_by_id(device.id)
        .one(db)
        .await
        .map_err(|_| Error::InternalServerError)?
    {
        return Ok(row);
    }
    let d = CameraSettings::default();
    let now: chrono::DateTime<chrono::FixedOffset> = chrono::Utc::now().into();
    device_cameras::ActiveModel {
        device_registry_id: Set(device.id),
        org_id: Set(device.org_id),
        framesize: Set(d.framesize.wire().to_string()),
        quality: Set(i16::from(d.quality)),
        fps: Set(i16::from(d.fps)),
        capture_mode: Set(d.capture_mode.wire().to_string()),
        vflip: Set(d.vflip),
        hmirror: Set(d.hmirror),
        created_at: Set(now),
        updated_at: Set(now),
    }
    .insert(db)
    .await
    .map_err(|_| Error::InternalServerError)
}

/// Is the uplink wanted right now? (D76 + D102: a deployed flow counts.)
pub fn demand(settings: &CameraSettings, viewers: u32, flow: bool) -> bool {
    settings.capture_mode == CaptureMode::Continuous || viewers > 0 || flow
}

/// Does a deployed flow of THIS pod read this camera? (D102)
pub fn flow_wants(org_id: i64, device_slug: &str) -> bool {
    let key = (org_id, device_slug.to_string());
    FLOW_DEMAND
        .lock()
        .expect("flow demand")
        .values()
        .any(|set| set.contains(&key))
}

/// Does a deployed flow of ANY pod read this camera? Local workers first,
/// then the live fields of [`FLOW_KEY`] (Valkey down = local answer).
pub async fn flow_wants_cluster(org_id: i64, device_slug: &str) -> bool {
    if flow_wants(org_id, device_slug) {
        return true;
    }
    let Some(cluster) = CLUSTER.get() else {
        return false;
    };
    let mut conn = cluster.conn.clone();
    let all: HashMap<String, String> = match conn.hgetall(FLOW_KEY).await {
        Ok(all) => all,
        Err(e) => {
            tracing::warn!(error = %e, "camera cluster: flow demand read failed (local view only)");
            return false;
        }
    };
    let now = now_ms();
    all.values()
        .filter_map(|v| serde_json::from_str::<FlowDemandEntry>(v).ok())
        .filter(|e| e.exp > now)
        .any(|e| e.c.iter().any(|(o, d)| *o == org_id && d == device_slug))
}

/// Live viewers of a camera on every pod (Valkey down = local count).
pub async fn viewers_cluster(device: i64) -> u32 {
    let local = HUB
        .lock()
        .expect("camera hub")
        .get(&device)
        .map_or(0, |e| e.viewers);
    let Some(cluster) = CLUSTER.get() else {
        return local;
    };
    let mut conn = cluster.conn.clone();
    let all: HashMap<String, String> = match conn.hgetall(VIEWERS_KEY).await {
        Ok(all) => all,
        Err(e) => {
            tracing::warn!(error = %e, "camera cluster: viewers read failed (local view only)");
            return local;
        }
    };
    let now = now_ms();
    local
        + all
            .iter()
            .filter_map(|(f, v)| Some((parse_viewer_field(f)?, parse_viewer_value(v)?)))
            .filter(|((d, pod), (_, exp))| *d == device && *pod != cluster.pod && *exp > now)
            .map(|(_, (n, _))| n)
            .sum::<u32>()
}

/// Is the uplink of this camera wanted right now, cluster-wide (settings,
/// viewers of every pod, flows of every pod)?
pub async fn wanted(
    settings: &CameraSettings,
    org_id: i64,
    device: i64,
    device_slug: &str,
) -> bool {
    if settings.capture_mode == CaptureMode::Continuous {
        return true;
    }
    let viewers = viewers_cluster(device).await;
    demand(
        settings,
        viewers,
        viewers == 0 && flow_wants_cluster(org_id, device_slug).await,
    )
}

/// Cameras read by the `camera-source` nodes of a flow artifact (the
/// runtime `flows.json`: node entries carry `device_id` + `pnex_org_id`).
/// Disabled nodes (`d: true`) do not count.
pub fn flow_demand_of_artifact(artifact: &serde_json::Value) -> HashSet<(i64, String)> {
    artifact
        .as_array()
        .into_iter()
        .flatten()
        .filter(|n| n.get("type").and_then(|t| t.as_str()) == Some("pnex-camera-source"))
        .filter(|n| n.get("d").and_then(|d| d.as_bool()) != Some(true))
        .filter_map(|n| {
            let org = n.get("pnex_org_id")?.as_i64()?;
            let slug = n.get("device_id")?.as_str()?.trim();
            (!slug.is_empty()).then(|| (org, slug.to_string()))
        })
        .collect()
}

/// Replaces the flow demand of one flow worker (`source`) and re-pushes
/// `CameraConfig` to every camera whose overall demand changed (no-op
/// before [`spawn_bus`] stored the DB).
pub fn set_flow_demand_from(source: &str, next: HashSet<(i64, String)>) {
    let changed: Vec<(i64, String)> = {
        let mut all = FLOW_DEMAND.lock().expect("flow demand");
        let union = |all: &HashMap<String, HashSet<(i64, String)>>| -> HashSet<(i64, String)> {
            all.values().flatten().cloned().collect()
        };
        let before = union(&all);
        if next.is_empty() {
            all.remove(source);
        } else {
            all.insert(source.to_string(), next);
        }
        let after = union(&all);
        before.symmetric_difference(&after).cloned().collect()
    };
    let Ok(handle) = tokio::runtime::Handle::try_current() else {
        return;
    };
    let source = source.to_string();
    let db = DB.get().cloned();
    handle.spawn(async move {
        // Cluster view first, so the re-push below reads it.
        write_flow_demand(&source).await;
        let Some(db) = db else {
            return;
        };
        for (org_id, slug) in changed {
            let found = device_registries::Entity::find()
                .filter(device_registries::Column::OrgId.eq(org_id))
                .filter(device_registries::Column::DeviceId.eq(slug.as_str()))
                .one(&db)
                .await;
            if let Ok(Some(dev)) = found {
                push_config(&db, dev.id).await;
            }
        }
    });
}

/// Pushes the current `CameraConfig` to the connected device (no-op when
/// offline — the announce pushes it again). Also arms the fps guard.
pub async fn push_config(db: &DatabaseConnection, device: i64) {
    let Ok(Some(row)) = device_cameras::Entity::find_by_id(device).one(db).await else {
        return;
    };
    let settings = settings_of(&row);
    arm_fps_guard(row.org_id, device, settings.fps);
    let enabled = match device_registries::Entity::find_by_id(device).one(db).await {
        Ok(Some(dev)) => wanted(&settings, row.org_id, device, &dev.device_id).await,
        _ => demand(&settings, viewers_cluster(device).await, false),
    };
    ws_device::push_command(device, config_msg(&settings, enabled)).await;
}

/// Server-side fps guard: +50 % margin over the configured rate (device
/// clock jitter), extra frames are dropped.
pub fn arm_fps_guard(org_id: i64, device: i64, fps: u8) {
    let fps = f64::from(fps.max(1));
    with_entry(device, org_id, |e| {
        e.min_interval_ms = (1000.0 / (fps * 1.5)) as i64
    });
}

pub fn config_msg(settings: &CameraSettings, enabled: bool) -> ServerMsg {
    ServerMsg::CameraConfig {
        cmd_id: uuid::Uuid::new_v4().simple().to_string(),
        enabled,
        framesize: settings.framesize.wire().to_string(),
        quality: settings.quality,
        fps: settings.fps,
        vflip: settings.vflip,
        hmirror: settings.hmirror,
    }
}

// ───────────────────────────── Viewers ─────────────────────────────

/// Live viewer registration — dropping the guard unregisters it.
pub struct ViewerGuard {
    db: DatabaseConnection,
    org_id: i64,
    device: i64,
}

/// Registers a live viewer; the first one (on this pod) wakes an
/// `on_demand` camera. With the cluster on, the viewer is also counted
/// cluster-wide and a bus relay feeds this pod when the uplink is remote.
pub async fn viewer_join(db: &DatabaseConnection, org_id: i64, device: i64) -> ViewerGuard {
    let first = with_entry(device, org_id, |e| {
        e.viewers += 1;
        e.generation += 1;
        e.viewers == 1
    });
    write_viewers(device).await;
    if CLUSTER.get().is_some() {
        if let Ok(Some(dev)) = device_registries::Entity::find_by_id(device).one(db).await {
            ensure_relay(org_id, device, &dev.device_id);
        }
    }
    if first {
        push_config(db, device).await;
    }
    ViewerGuard {
        db: db.clone(),
        org_id,
        device,
    }
}

impl Drop for ViewerGuard {
    fn drop(&mut self) {
        let (last, generation) = with_entry(self.device, self.org_id, |e| {
            e.viewers = e.viewers.saturating_sub(1);
            (e.viewers == 0, e.generation)
        });
        if !last {
            if CLUSTER.get().is_some() {
                if let Ok(handle) = tokio::runtime::Handle::try_current() {
                    handle.spawn(write_viewers(self.device));
                }
            }
            return;
        }
        let db = self.db.clone();
        let (org_id, device) = (self.org_id, self.device);
        // Drop may run outside a runtime (process teardown): skip then.
        let Ok(handle) = tokio::runtime::Handle::try_current() else {
            return;
        };
        handle.spawn(async move {
            // Leave the cluster count now; the grace below re-reads it.
            write_viewers(device).await;
            tokio::time::sleep(Duration::from_secs(ON_DEMAND_GRACE_SECS)).await;
            let still_idle = with_entry(device, org_id, |e| {
                e.viewers == 0 && e.generation == generation
            });
            if still_idle {
                push_config(&db, device).await;
            }
        });
    }
}

// ───────────────────────────── Cluster ─────────────────────────────

/// Viewer presence, all cameras: field `{device}|{pod}`, value
/// `{count}|{expiry unix ms}`.
pub const VIEWERS_KEY: &str = "pnex:camview:v1";
/// Flow demand, all workers: field = flow worker id, value = JSON
/// [`FlowDemandEntry`].
pub const FLOW_KEY: &str = "pnex:camflow:v1";
/// Lifetime of a presence / demand field without refresh.
const PRESENCE_TTL_MS: i64 = 60_000;
/// Heartbeat period (refresh of this pod's fields + pruning of dead ones).
const HEARTBEAT: Duration = Duration::from_secs(20);
/// Uplink claim lifetime; the uplink session refreshes it every
/// [`UPLINK_CLAIM_REFRESH`].
const UPLINK_CLAIM_TTL_SECS: u64 = 45;
pub const UPLINK_CLAIM_REFRESH: Duration = Duration::from_secs(15);
/// Latest frame metadata lifetime (`last_frame_ms` of the camera list).
const LATEST_TTL_SECS: u64 = 24 * 3600;

struct Cluster {
    conn: ConnectionManager,
    url: String,
    pod: String,
}

/// One cluster state per tokio runtime (see `runtime_local`).
static CLUSTER: crate::services::runtime_local::PerRuntime<Arc<Cluster>> =
    crate::services::runtime_local::PerRuntime::new();

/// Cross-pod uplink claim of a camera: `<pod>|<session>`.
pub fn uplink_key(device: i64) -> String {
    format!("pnex:camup:v1:{device}")
}

/// Metadata (JSON [`BusFrameMeta`]) of the last frame of a camera.
pub fn latest_key(org_id: i64, device_slug: &str) -> String {
    format!("pnex:cam:v1:{org_id}:{device_slug}:latest")
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct FlowDemandEntry {
    /// Expiry, unix ms.
    pub exp: i64,
    /// Cameras wanted: `(org, device slug)`.
    pub c: Vec<(i64, String)>,
}

fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

pub fn viewer_field(device: i64, pod: &str) -> String {
    format!("{device}|{pod}")
}

fn parse_viewer_field(field: &str) -> Option<(i64, &str)> {
    let (device, pod) = field.split_once('|')?;
    Some((device.parse().ok()?, pod))
}

fn parse_viewer_value(value: &str) -> Option<(u32, i64)> {
    let (n, exp) = value.split_once('|')?;
    Some((n.parse().ok()?, exp.parse().ok()?))
}

/// Deletes a hash field only if it still holds `value` (a refresh by its
/// owner in between wins). Returns whether it was deleted.
async fn hdel_if(conn: &mut ConnectionManager, key: &str, field: &str, value: &str) -> bool {
    redis::Script::new(
        "if redis.call('HGET', KEYS[1], ARGV[1]) == ARGV[2] then return redis.call('HDEL', KEYS[1], ARGV[1]) else return 0 end",
    )
    .key(key)
    .arg(field)
    .arg(value)
    .invoke_async::<i64>(conn)
    .await
    .unwrap_or(0)
        > 0
}

/// Publishes this pod's local viewer count of a camera (0 = field removed).
async fn write_viewers(device: i64) {
    let Some(cluster) = CLUSTER.get() else {
        return;
    };
    let n = HUB
        .lock()
        .expect("camera hub")
        .get(&device)
        .map_or(0, |e| e.viewers);
    let field = viewer_field(device, &cluster.pod);
    let mut conn = cluster.conn.clone();
    let res: redis::RedisResult<()> = if n == 0 {
        conn.hdel(VIEWERS_KEY, field).await
    } else {
        conn.hset(
            VIEWERS_KEY,
            field,
            format!("{n}|{}", now_ms() + PRESENCE_TTL_MS),
        )
        .await
    };
    if let Err(e) = res {
        tracing::warn!(device, error = %e, "camera cluster: viewer presence write failed");
    }
}

/// Publishes the demand of a local flow worker (empty = field removed).
async fn write_flow_demand(source: &str) {
    let Some(cluster) = CLUSTER.get() else {
        return;
    };
    let cameras: Vec<(i64, String)> = FLOW_DEMAND
        .lock()
        .expect("flow demand")
        .get(source)
        .map(|set| set.iter().cloned().collect())
        .unwrap_or_default();
    let mut conn = cluster.conn.clone();
    let res: redis::RedisResult<()> = if cameras.is_empty() {
        conn.hdel(FLOW_KEY, source).await
    } else {
        let entry = FlowDemandEntry {
            exp: now_ms() + PRESENCE_TTL_MS,
            c: cameras,
        };
        match serde_json::to_string(&entry) {
            Ok(v) => conn.hset(FLOW_KEY, source, v).await,
            Err(_) => return,
        }
    };
    if let Err(e) = res {
        tracing::warn!(worker = source, error = %e, "camera cluster: flow demand write failed");
    }
}

/// Per-pod heartbeat: refreshes this pod's viewer and flow fields, prunes
/// the expired fields of dead pods / workers and re-pushes the config of
/// the cameras they were keeping awake.
async fn heartbeat_loop() {
    let mut tick = tokio::time::interval(HEARTBEAT);
    loop {
        tick.tick().await;
        heartbeat_once().await;
    }
}

/// One heartbeat round (public for tests).
pub async fn heartbeat_once() {
    let Some(cluster) = CLUSTER.get() else {
        return;
    };
    let watched: Vec<i64> = HUB
        .lock()
        .expect("camera hub")
        .iter()
        .filter(|(_, e)| e.viewers > 0)
        .map(|(d, _)| *d)
        .collect();
    for device in watched {
        write_viewers(device).await;
    }
    let sources: Vec<String> = FLOW_DEMAND
        .lock()
        .expect("flow demand")
        .keys()
        .cloned()
        .collect();
    for source in sources {
        write_flow_demand(&source).await;
    }
    let mut conn = cluster.conn.clone();
    let now = now_ms();
    let mut repush_devices: HashSet<i64> = HashSet::new();
    let mut repush_slugs: HashSet<(i64, String)> = HashSet::new();
    if let Ok(all) = conn
        .hgetall::<_, HashMap<String, String>>(VIEWERS_KEY)
        .await
    {
        for (field, value) in all {
            let Some(((device, _), (_, exp))) =
                parse_viewer_field(&field).zip(parse_viewer_value(&value))
            else {
                continue;
            };
            if exp <= now && hdel_if(&mut conn, VIEWERS_KEY, &field, &value).await {
                repush_devices.insert(device);
            }
        }
    }
    if let Ok(all) = conn.hgetall::<_, HashMap<String, String>>(FLOW_KEY).await {
        for (field, value) in all {
            let Ok(entry) = serde_json::from_str::<FlowDemandEntry>(&value) else {
                continue;
            };
            if entry.exp <= now && hdel_if(&mut conn, FLOW_KEY, &field, &value).await {
                repush_slugs.extend(entry.c);
            }
        }
    }
    let Some(db) = DB.get() else {
        return;
    };
    for (org_id, slug) in repush_slugs {
        let found = device_registries::Entity::find()
            .filter(device_registries::Column::OrgId.eq(org_id))
            .filter(device_registries::Column::DeviceId.eq(slug.as_str()))
            .one(db)
            .await;
        if let Ok(Some(dev)) = found {
            repush_devices.insert(dev.id);
        }
    }
    for device in repush_devices {
        tracing::info!(
            device,
            "camera cluster: stale presence pruned — config re-pushed"
        );
        push_config(db, device).await;
    }
}

/// Claims the uplink of a camera for `session` across pods. `false` =
/// another live session (any pod) holds it. Valkey absent or failing =
/// claim granted (the per-pod registry still guards the local pod).
pub async fn claim_uplink(device: i64, session: &str) -> bool {
    let Some(cluster) = CLUSTER.get() else {
        return true;
    };
    let value = format!("{}|{session}", cluster.pod);
    let mut conn = cluster.conn.clone();
    let res: redis::RedisResult<Option<String>> = redis::cmd("SET")
        .arg(uplink_key(device))
        .arg(&value)
        .arg("NX")
        .arg("EX")
        .arg(UPLINK_CLAIM_TTL_SECS)
        .query_async(&mut conn)
        .await;
    match res {
        Ok(Some(_)) => true,
        Ok(None) => {
            let holder: Option<String> = conn.get(uplink_key(device)).await.ok().flatten();
            holder.as_deref() == Some(value.as_str())
        }
        Err(e) => {
            tracing::warn!(device, error = %e, "camera cluster: uplink claim failed — granted locally");
            true
        }
    }
}

/// Refreshes the uplink claim of `session`. `false` = the claim now
/// belongs to another session (the caller closes its uplink). An expired
/// claim is taken back.
pub async fn refresh_uplink(device: i64, session: &str) -> bool {
    let Some(cluster) = CLUSTER.get() else {
        return true;
    };
    let mut conn = cluster.conn.clone();
    let res: redis::RedisResult<i64> = redis::Script::new(
        "local v = redis.call('GET', KEYS[1]) \
         if v == ARGV[1] then redis.call('EXPIRE', KEYS[1], ARGV[2]) return 1 \
         elseif not v then redis.call('SET', KEYS[1], ARGV[1], 'EX', ARGV[2]) return 1 \
         else return 0 end",
    )
    .key(uplink_key(device))
    .arg(format!("{}|{session}", cluster.pod))
    .arg(UPLINK_CLAIM_TTL_SECS)
    .invoke_async(&mut conn)
    .await;
    match res {
        Ok(n) => n == 1,
        Err(e) => {
            tracing::warn!(device, error = %e, "camera cluster: uplink claim refresh failed — kept");
            true
        }
    }
}

/// Releases the uplink claim if `session` still holds it.
pub async fn release_uplink(device: i64, session: &str) {
    let Some(cluster) = CLUSTER.get() else {
        return;
    };
    let mut conn = cluster.conn.clone();
    let res: redis::RedisResult<i64> = redis::Script::new(
        "if redis.call('GET', KEYS[1]) == ARGV[1] then return redis.call('DEL', KEYS[1]) else return 0 end",
    )
    .key(uplink_key(device))
    .arg(format!("{}|{session}", cluster.pod))
    .invoke_async(&mut conn)
    .await;
    if let Err(e) = res {
        tracing::warn!(device, error = %e, "camera cluster: uplink release failed");
    }
}

/// Starts the bus relay of a camera on this pod if none runs: frames of a
/// remote uplink feed the local live broadcast while local viewers remain.
fn ensure_relay(org_id: i64, device: i64, device_slug: &str) {
    let Some(cluster) = CLUSTER.get() else {
        return;
    };
    let start = with_entry(device, org_id, |e| {
        if e.relay || e.viewers == 0 {
            return false;
        }
        e.relay = true;
        true
    });
    if start {
        tokio::spawn(relay_loop(
            cluster.url.clone(),
            org_id,
            device,
            device_slug.to_string(),
        ));
    }
}

/// Should the relay keep running? Stops (and says so atomically) when the
/// last local viewer left.
fn relay_keeps_running(org_id: i64, device: i64) -> bool {
    with_entry(device, org_id, |e| {
        if e.viewers == 0 {
            e.relay = false;
            return false;
        }
        true
    })
}

async fn relay_loop(url: String, org_id: i64, device: i64, slug: String) {
    let channel = pnex_core::camera::bus_channel(org_id, &slug);
    let mut backoff = Duration::from_millis(500);
    'outer: while relay_keeps_running(org_id, device) {
        let pubsub = match redis::Client::open(url.as_str()) {
            Ok(client) => client.get_async_pubsub().await,
            Err(e) => {
                tracing::error!(error = %e, "camera relay: invalid valkey url");
                with_entry(device, org_id, |e| e.relay = false);
                return;
            }
        };
        let mut pubsub = match pubsub {
            Ok(p) => p,
            Err(e) => {
                tracing::warn!(device, error = %e, "camera relay: pubsub connection failed");
                tokio::time::sleep(backoff).await;
                backoff = (backoff * 2).min(Duration::from_secs(10));
                continue;
            }
        };
        if let Err(e) = pubsub.subscribe(&channel).await {
            tracing::warn!(device, error = %e, "camera relay: subscribe failed");
            tokio::time::sleep(backoff).await;
            continue;
        }
        backoff = Duration::from_millis(500);
        tracing::debug!(device, %channel, "camera relay subscribed");
        let mut stream = pubsub.on_message();
        loop {
            let next = tokio::time::timeout(Duration::from_secs(2), stream.next()).await;
            if !relay_keeps_running(org_id, device) {
                break 'outer;
            }
            let msg = match next {
                Err(_) => continue, // idle camera: re-check the viewers
                Ok(None) => break,  // connection lost: resubscribe
                Ok(Some(msg)) => msg,
            };
            // A local uplink already feeds the broadcast.
            if with_entry(device, org_id, |e| e.uplink) {
                continue;
            }
            let Ok(payload) = msg.get_payload::<String>() else {
                continue;
            };
            let Ok(meta) = serde_json::from_str::<BusFrameMeta>(&payload) else {
                continue;
            };
            let Some(cluster) = CLUSTER.get() else {
                continue;
            };
            let mut conn = cluster.conn.clone();
            let jpeg: Option<Vec<u8>> = conn.get(&meta.key).await.ok().flatten();
            let Some(jpeg) = jpeg else {
                continue;
            };
            let frame = Arc::new(frame_of_meta(org_id, device, &slug, &meta, jpeg));
            with_entry(device, org_id, |e| {
                e.latest = Some(frame.clone());
                let _ = e.tx.send(frame);
            });
        }
    }
    tracing::debug!(device, "camera relay stopped");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn demand_rules() {
        let mut s = CameraSettings::default();
        assert!(!demand(&s, 0, false));
        assert!(demand(&s, 1, false));
        assert!(
            demand(&s, 0, true),
            "a deployed flow wakes an on_demand camera"
        );
        s.capture_mode = CaptureMode::Continuous;
        assert!(demand(&s, 0, false));
    }

    #[test]
    fn flow_demand_reads_camera_sources_of_the_artifact() {
        let artifact = serde_json::json!([
            {"type": "tab", "id": "pnexflow1"},
            {"type": "pnex-camera-source", "device_id": "proud-robin", "pnex_org_id": 1},
            {"type": "pnex-camera-source", "device_id": "off-cam", "pnex_org_id": 1, "d": true},
            {"type": "pnex-camera-source", "device_id": "", "pnex_org_id": 1},
            {"type": "pnex-vision-detect", "device_id": "not-a-source", "pnex_org_id": 1},
        ]);
        let got = flow_demand_of_artifact(&artifact);
        assert_eq!(got, HashSet::from([(1, "proud-robin".to_string())]));
    }

    #[tokio::test]
    async fn publish_fans_out_and_guards_fps() {
        let device = 987_654;
        let slug: Arc<str> = Arc::from("cam-test");
        let (_, mut rx) = subscribe(1, device);
        let h = FrameHeader {
            seq: 1,
            uptime_ms: 0,
            width: 2,
            height: 2,
        };
        assert!(publish(
            1,
            device,
            &slug,
            h,
            Bytes::from_static(&[0xFF, 0xD8, 1])
        ));
        assert_eq!(rx.recv().await.unwrap().header.seq, 1);
        with_entry(device, 1, |e| e.min_interval_ms = 10_000);
        assert!(!publish(
            1,
            device,
            &slug,
            FrameHeader { seq: 2, ..h },
            Bytes::from_static(&[0xFF, 0xD8])
        ));
        assert_eq!(latest(device).unwrap().header.seq, 1);
    }

    /// Cluster primitives against a real Valkey (`PNEX_TEST_VALKEY_URL`,
    /// skipped otherwise): another pod is simulated by raw writes.
    #[tokio::test]
    async fn cluster_claims_presence_demand_and_relay() {
        let Ok(url) = std::env::var("PNEX_TEST_VALKEY_URL") else {
            eprintln!("PNEX_TEST_VALKEY_URL unset — camera cluster test skipped");
            return;
        };
        let suffix = std::process::id();
        let pod = format!("test-cam-a-{suffix}");
        let other = format!("test-cam-b-{suffix}");
        let client = redis::Client::open(url.as_str()).unwrap();
        let mut kv = ConnectionManager::new(client).await.unwrap();
        start_cluster(kv.clone(), url.clone(), pod.clone());
        let device = 970_000_000 + i64::from(suffix % 100_000);
        let org = 42;
        let slug = format!("cam-unit-{suffix}");

        // Uplink claim: owner-checked, cross-pod exclusive.
        let _: () = kv.del(uplink_key(device)).await.unwrap();
        assert!(claim_uplink(device, "s1").await);
        assert!(claim_uplink(device, "s1").await, "re-claim by the holder");
        assert!(!claim_uplink(device, "s2").await, "second session refused");
        assert!(refresh_uplink(device, "s1").await);
        release_uplink(device, "s2").await; // not the holder: no-op
        assert!(!claim_uplink(device, "s2").await);
        release_uplink(device, "s1").await;
        assert!(claim_uplink(device, "s2").await, "free after release");
        let _: () = kv
            .set(uplink_key(device), format!("{other}|x"))
            .await
            .unwrap();
        assert!(
            !refresh_uplink(device, "s2").await,
            "claim lost to another pod"
        );
        let _: () = kv.del(uplink_key(device)).await.unwrap();

        // Viewers: remote live fields count, expired ones do not and are
        // pruned by the heartbeat.
        let future = now_ms() + 60_000;
        let field = viewer_field(device, &other);
        let _: () = kv
            .hset(VIEWERS_KEY, &field, format!("2|{future}"))
            .await
            .unwrap();
        assert_eq!(viewers_cluster(device).await, 2);
        let _: () = kv.hset(VIEWERS_KEY, &field, "2|1").await.unwrap();
        assert_eq!(viewers_cluster(device).await, 0);
        heartbeat_once().await;
        let left: Option<String> = kv.hget(VIEWERS_KEY, &field).await.unwrap();
        assert_eq!(left, None, "expired field pruned");

        // Flow demand: local workers and remote fields.
        let wfield = format!("test-worker-{suffix}");
        assert!(!flow_wants_cluster(org, &slug).await);
        let entry = FlowDemandEntry {
            exp: future,
            c: vec![(org, slug.clone())],
        };
        let _: () = kv
            .hset(FLOW_KEY, &wfield, serde_json::to_string(&entry).unwrap())
            .await
            .unwrap();
        assert!(flow_wants_cluster(org, &slug).await);
        let _: () = kv.hdel(FLOW_KEY, &wfield).await.unwrap();
        set_flow_demand_from(&wfield, HashSet::from([(org, slug.clone())]));
        let mut written = false;
        for _ in 0..50 {
            let v: Option<String> = kv.hget(FLOW_KEY, &wfield).await.unwrap();
            if v.is_some() {
                written = true;
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        assert!(written, "local worker demand published");
        set_flow_demand_from(&wfield, HashSet::new());
        tokio::time::sleep(Duration::from_millis(200)).await;
        assert!(!flow_wants_cluster(org, &slug).await);

        // Relay: a local viewer of a remote uplink gets the bus frames.
        with_entry(device, org, |e| e.viewers += 1);
        let (_, mut rx) = subscribe(org, device);
        ensure_relay(org, device, &slug);
        let channel = pnex_core::camera::bus_channel(org, &slug);
        for _ in 0..100 {
            let n: Vec<(String, i64)> = redis::cmd("PUBSUB")
                .arg("NUMSUB")
                .arg(&channel)
                .query_async(&mut kv)
                .await
                .unwrap();
            if n.first().is_some_and(|(_, n)| *n > 0) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        let key = pnex_core::camera::frame_key(org, &slug, 9);
        let meta = BusFrameMeta {
            seq: 9,
            ts_ms: now_ms(),
            width: 2,
            height: 2,
            size: 3,
            key: key.clone(),
        };
        let meta = serde_json::to_string(&meta).unwrap();
        let _: () = kv.set_ex(&key, vec![0xFFu8, 0xD8, 9], 15).await.unwrap();
        let _: () = kv.set_ex(latest_key(org, &slug), &meta, 15).await.unwrap();
        let _: i64 = kv.publish(&channel, &meta).await.unwrap();
        let got = tokio::time::timeout(Duration::from_secs(3), rx.recv())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(got.header.seq, 9);
        assert_eq!(&got.jpeg[..], &[0xFF, 0xD8, 9]);
        // Last viewer gone: the relay stops.
        with_entry(device, org, |e| e.viewers -= 1);
        let mut stopped = false;
        for _ in 0..50 {
            if !with_entry(device, org, |e| e.relay) {
                stopped = true;
                break;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        assert!(stopped, "relay stopped with the last viewer");

        // Snapshot from any pod: the bus latest frame.
        let other_device = device + 1;
        let frame = latest_anywhere(org, other_device, &slug)
            .await
            .expect("bus frame");
        assert_eq!(frame.header.seq, 9);
        let states = live_states(&[(other_device, org, slug.as_str())]).await;
        assert!(states[&other_device].last_frame_ms.is_some());
        assert!(!states[&other_device].uplink);
    }
}
