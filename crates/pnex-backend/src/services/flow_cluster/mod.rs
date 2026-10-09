//! Flow execution cluster (D106, docs/architecture/flow-engine.md §12).
//!
//! Split between a **control plane** and a **data plane**:
//! - control plane: one elected placement controller (row lease
//!   `flow-placement`) decides which worker runs the flows of which org —
//!   failover of dead workers, draining, bounded rebalancing
//!   ([`placement::plan`]). It never runs a flow.
//! - data plane: N workers. A worker = one supervisor = one runtime child
//!   running the flows of the orgs placed on it. It heartbeats, fences
//!   itself when it loses its registration, and reconciles its runtime
//!   from the database (O(own orgs) per tick, driven by the placement
//!   `revision`).
//!
//! The API side never talks to a runtime directly: [`apply_org`] projects
//! the flows of ONE org (never the whole instance) and hands the fragment
//! to the owner worker — in-process when the owner is this process
//! (embedded mode = a cluster of one), over the internal HTTP routes
//! otherwise ([`transport`]).
//!
//! Fencing: every placement move bumps its epoch; the runtime stamps its
//! worker identity (`<id>:<boot>`) on the internal write routes and the
//! backend rejects writes of a worker that no longer owns the org
//! ([`fence_ok`]).

pub mod controller;
pub mod placement;
pub mod store;
pub mod transport;
pub mod worker;

use std::sync::Arc;
use std::time::Duration;

use loco_rs::config::Config;
use loco_rs::prelude::AppContext;
use sea_orm::DatabaseConnection;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::services::flow::FlowSettings;
use crate::services::flow_supervisor::{DeployError, FlowCheckError};
use pnex_core::{FlowArtifactMeta, FlowDebugEntry, FlowRuntimeStatus};

pub use worker::WorkerNode;

/// Name of the placement controller lease.
pub const PLACEMENT_LEASE: &str = "flow-placement";

/// Resolved `settings.flow.cluster` (every field optional in the yaml).
#[derive(Clone, Debug)]
pub struct ClusterSettings {
    /// This process executes flows (a worker). `false` = API-only pod
    /// (still eligible to run the placement controller).
    pub worker: bool,
    /// Stable worker id. Default: `PNEX_FLOW_WORKER_ID`, else the host name
    /// (the pod name under Kubernetes), else `local`.
    pub worker_id: String,
    /// Base URL other pods use to reach the internal cluster routes of
    /// this process (e.g. `http://$(POD_IP):5150`). Empty = in-process
    /// only (embedded single-node mode).
    pub advertise_url: String,
    /// Shared secret of the internal cluster routes (`x-pnex-cluster-token`).
    /// Empty = routes closed (401), fail-closed.
    pub token: String,
    /// Max deployed flows accepted by this worker (0 = unbounded).
    pub capacity: u32,
    pub heartbeat_ms: u64,
    /// A worker without heartbeat for this long is dead (failover).
    pub worker_ttl_ms: u64,
    /// A worker that could not heartbeat for this long kills its runtime
    /// (must stay below `worker_ttl_ms`: it stops before being replaced).
    pub self_fence_ms: u64,
    pub controller_tick_ms: u64,
    pub lease_ttl_ms: u64,
    /// Refresh period of the org weights (deployed flows per org).
    pub weights_refresh_ms: u64,
    /// Unconditional reprojection of every owned org (heals a missed
    /// revision bump or a crash between an ack and its database write).
    pub full_resync_ms: u64,
    /// Rebalance moves per controller round.
    pub max_moves: usize,
    /// Dead worker rows owning nothing are collected after this delay.
    pub gc_ms: u64,
    /// An org without deployed flows keeps its placement at least this long
    /// after its last change (covers a deploy in flight: placement created,
    /// database not marked yet).
    pub release_grace_ms: u64,
    /// Graceful shutdown budget to hand the orgs over.
    pub drain_timeout_ms: u64,
    /// Timeout of an internal call to another worker.
    pub request_timeout_ms: u64,
}

impl Default for ClusterSettings {
    fn default() -> Self {
        Self {
            worker: true,
            worker_id: default_worker_id(),
            advertise_url: String::new(),
            token: String::new(),
            capacity: 0,
            heartbeat_ms: 2_000,
            worker_ttl_ms: 10_000,
            self_fence_ms: 6_000,
            controller_tick_ms: 2_000,
            lease_ttl_ms: 10_000,
            weights_refresh_ms: 30_000,
            full_resync_ms: 300_000,
            max_moves: 20,
            gc_ms: 600_000,
            release_grace_ms: 120_000,
            drain_timeout_ms: 20_000,
            request_timeout_ms: 30_000,
        }
    }
}

fn default_worker_id() -> String {
    let from_env = |k: &str| std::env::var(k).ok().map(|v| v.trim().to_string());
    from_env("PNEX_FLOW_WORKER_ID")
        .filter(|v| !v.is_empty())
        .or_else(|| from_env("HOSTNAME").filter(|v| !v.is_empty()))
        .or_else(|| {
            std::fs::read_to_string("/proc/sys/kernel/hostname")
                .ok()
                .map(|v| v.trim().to_string())
                .filter(|v| !v.is_empty())
        })
        .unwrap_or_else(|| "local".into())
}

#[derive(Default, Deserialize)]
struct ClusterPartial {
    worker: Option<bool>,
    worker_id: Option<String>,
    advertise_url: Option<String>,
    token: Option<String>,
    capacity: Option<u32>,
    heartbeat_ms: Option<u64>,
    worker_ttl_ms: Option<u64>,
    self_fence_ms: Option<u64>,
    controller_tick_ms: Option<u64>,
    lease_ttl_ms: Option<u64>,
    weights_refresh_ms: Option<u64>,
    full_resync_ms: Option<u64>,
    max_moves: Option<usize>,
    gc_ms: Option<u64>,
    release_grace_ms: Option<u64>,
    drain_timeout_ms: Option<u64>,
    request_timeout_ms: Option<u64>,
}

impl ClusterSettings {
    pub fn from_config(config: &Config) -> Self {
        let p: ClusterPartial = config
            .settings
            .as_ref()
            .and_then(|s| s.get("flow"))
            .and_then(|f| f.get("cluster"))
            .and_then(|v| serde_json::from_value(v.clone()).ok())
            .unwrap_or_default();
        let d = Self::default();
        let s = Self {
            worker: p.worker.unwrap_or(d.worker),
            worker_id: p
                .worker_id
                .map(|v| v.trim().to_string())
                .filter(|v| !v.is_empty())
                .unwrap_or(d.worker_id),
            advertise_url: p
                .advertise_url
                .map(|v| v.trim().trim_end_matches('/').to_string())
                .unwrap_or(d.advertise_url),
            token: p.token.map(|v| v.trim().to_string()).unwrap_or(d.token),
            capacity: p.capacity.unwrap_or(d.capacity),
            heartbeat_ms: p.heartbeat_ms.unwrap_or(d.heartbeat_ms),
            worker_ttl_ms: p.worker_ttl_ms.unwrap_or(d.worker_ttl_ms),
            self_fence_ms: p.self_fence_ms.unwrap_or(d.self_fence_ms),
            controller_tick_ms: p.controller_tick_ms.unwrap_or(d.controller_tick_ms),
            lease_ttl_ms: p.lease_ttl_ms.unwrap_or(d.lease_ttl_ms),
            weights_refresh_ms: p.weights_refresh_ms.unwrap_or(d.weights_refresh_ms),
            full_resync_ms: p.full_resync_ms.unwrap_or(d.full_resync_ms),
            max_moves: p.max_moves.unwrap_or(d.max_moves),
            gc_ms: p.gc_ms.unwrap_or(d.gc_ms),
            release_grace_ms: p.release_grace_ms.unwrap_or(d.release_grace_ms),
            drain_timeout_ms: p.drain_timeout_ms.unwrap_or(d.drain_timeout_ms),
            request_timeout_ms: p.request_timeout_ms.unwrap_or(d.request_timeout_ms),
        };
        s.sanitized()
    }

    /// Enforces the timing invariants the safety relies on.
    pub fn sanitized(mut self) -> Self {
        self.heartbeat_ms = self.heartbeat_ms.max(50);
        // Several heartbeats per TTL, self-fencing strictly before the TTL.
        self.worker_ttl_ms = self.worker_ttl_ms.max(self.heartbeat_ms * 3);
        if self.self_fence_ms == 0 || self.self_fence_ms >= self.worker_ttl_ms {
            self.self_fence_ms = self.worker_ttl_ms * 3 / 5;
        }
        self.self_fence_ms = self.self_fence_ms.max(self.heartbeat_ms * 2);
        self.controller_tick_ms = self.controller_tick_ms.max(50);
        self.lease_ttl_ms = self.lease_ttl_ms.max(self.controller_tick_ms * 3);
        self
    }
}

/// One org fragment handed to its owner worker.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApplyOrg {
    pub org_id: i64,
    /// The complete projected entries of the org (all its flows).
    pub fragment: Value,
    /// Acknowledging flow (deploy/stop of one flow). `None` = reprojection.
    pub ack: Option<FlowArtifactMeta>,
    /// Flow whose pre-flight failure is blocking.
    pub enforce_flow: Option<i64>,
}

/// Outcome of an [`ApplyOrg`] on a worker. Externally tagged on purpose:
/// an internally tagged enum buffers its fields, which breaks numbers once
/// `serde_json/arbitrary_precision` is unified into the build.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ApplyError {
    /// The worker does not own the org (moved placement): re-resolve.
    NotOwner,
    /// The owner could not be reached (dead pod, network): retry later.
    Unreachable {
        error: String,
    },
    /// Blocking pre-flight failures.
    Check {
        checks: Vec<FlowCheckError>,
    },
    Runtime {
        error: String,
    },
}

impl From<DeployError> for ApplyError {
    fn from(e: DeployError) -> Self {
        match e {
            DeployError::Check(checks) => Self::Check { checks },
            DeployError::Runtime(error) => Self::Runtime { error },
        }
    }
}

/// Boots the cluster roles of this process (from `App::after_routes`):
/// the worker (when `settings.flow.enabled` and `cluster.worker`) and the
/// placement controller candidate (whenever the flow engine is enabled).
pub async fn boot(ctx: &AppContext) {
    let flow = FlowSettings::from_config(&ctx.config);
    if !flow.enabled {
        tracing::info!("flow engine disabled (settings.flow.enabled=false)");
        return;
    }
    let cfg = ClusterSettings::from_config(&ctx.config);
    warn_incomplete_setup(&ctx.config, &cfg);
    let holder = if cfg.worker {
        match WorkerNode::start(ctx.db.clone(), flow, cfg.clone()).await {
            Ok(node) => {
                tracing::info!(worker = %node.id, boot = node.boot, "flow worker started");
                format!("{}:{}", node.id, node.boot)
            }
            Err(e) => {
                tracing::error!(error = %e, "flow worker registration failed — flows will not run here");
                return;
            }
        }
    } else {
        format!("api:{}:{}", cfg.worker_id, worker::new_boot())
    };
    controller::spawn(ctx.db.clone(), cfg, holder);
}

/// Multi-pod deployments need a reachable `advertise_url` and a shared
/// token; a half-configured cluster only shows up as 503s at the first
/// cross-pod deploy — say it at boot instead.
fn warn_incomplete_setup(config: &Config, cfg: &ClusterSettings) {
    let valkey = crate::services::last_cache::ValkeySettings::from_config(config)
        .and_then(|v| v.url)
        .is_some_and(|u| !u.trim().is_empty());
    if !cfg.advertise_url.is_empty() && cfg.token.is_empty() {
        tracing::warn!("flow cluster: advertise_url is set but the cluster token is empty — other pods will get 401 (set PNEX_FLOW_CLUSTER_TOKEN)");
    }
    if cfg.advertise_url.is_empty() && valkey {
        tracing::info!(
            worker = %cfg.worker_id,
            "flow cluster: no advertise_url — fine for a single node; with several pods set PNEX_FLOW_ADVERTISE_URL"
        );
    }
    if !cfg.advertise_url.is_empty() && !valkey {
        tracing::warn!("flow cluster: several pods without Valkey — device commands only reach devices connected to the same pod (D107)");
    }
}

/// Graceful shutdown (from `App::on_shutdown`): drain the local worker.
pub async fn shutdown() {
    for node in transport::local_workers() {
        node.shutdown(true).await;
    }
    controller::release_all().await;
}

// ───────────────────────────── API side ─────────────────────────────

/// Owner of `org_id`, creating its placement on the least loaded live
/// worker when the org has none yet (first deploy of the org).
pub async fn ensure_owner(
    db: &DatabaseConnection,
    org_id: i64,
    cfg: &ClusterSettings,
) -> Result<String, DeployError> {
    let db_err = |e: sea_orm::DbErr| DeployError::Runtime(format!("flow cluster: {e}"));
    if let Some(p) = store::placement(db, org_id).await.map_err(db_err)? {
        return Ok(p.worker_id);
    }
    let workers = store::workers(db).await.map_err(db_err)?;
    let placements = store::placements(db).await.map_err(db_err)?;
    let mut load: std::collections::HashMap<&str, u64> = workers
        .iter()
        .filter(|w| !w.draining && store::is_alive(w, cfg.worker_ttl_ms))
        .map(|w| (w.id.as_str(), 0))
        .collect();
    for p in &placements {
        if let Some(l) = load.get_mut(p.worker_id.as_str()) {
            *l += 1;
        }
    }
    let Some(target) = load
        .iter()
        .min_by_key(|(id, l)| (**l, id.to_string()))
        .map(|(id, _)| id.to_string())
    else {
        return Err(DeployError::Runtime(
            "no flow worker available (settings.flow.enabled ?)".into(),
        ));
    };
    store::insert_placement(db, org_id, &target)
        .await
        .map_err(db_err)?;
    // A concurrent creator may have won: the stored row is the truth.
    store::placement(db, org_id)
        .await
        .map_err(db_err)?
        .map(|p| p.worker_id)
        .ok_or_else(|| DeployError::Runtime("flow placement vanished".into()))
}

/// Hands the projected fragment of `org_id` to its owner and waits for the
/// runtime acknowledgement (`ack`). Retries a moved placement or an owner
/// being replaced for a few seconds, then answers 503-shaped errors.
pub async fn apply_org(
    db: &DatabaseConnection,
    config: &Config,
    req: ApplyOrg,
) -> Result<(), DeployError> {
    let cfg = ClusterSettings::from_config(config);
    let mut last = String::new();
    for attempt in 0..4u64 {
        if attempt > 0 {
            tokio::time::sleep(Duration::from_millis(300 * attempt)).await;
        }
        let owner = ensure_owner(db, req.org_id, &cfg).await?;
        match transport::apply(db, &cfg, &owner, &req).await {
            Ok(()) => return Ok(()),
            Err(ApplyError::Check { checks }) => return Err(DeployError::Check(checks)),
            Err(ApplyError::Runtime { error }) => return Err(DeployError::Runtime(error)),
            Err(ApplyError::NotOwner) => {
                last = format!("worker {owner} no longer owns org {}", req.org_id)
            }
            Err(ApplyError::Unreachable { error }) => {
                last = format!("worker {owner} unreachable: {error}")
            }
        }
    }
    Err(DeployError::Runtime(format!(
        "flow runtime unavailable ({last})"
    )))
}

/// The projected flows of `org_id` changed in the database: its owner
/// reprojects it at its next tick. Best effort (the periodic full resync
/// heals a missed bump).
pub async fn org_changed(db: &DatabaseConnection, org_id: i64) {
    if let Err(e) = store::bump_revision(db, org_id).await {
        tracing::warn!(org_id, error = %e, "flow cluster: revision bump failed");
    }
}

/// Runtime state of one flow, asked to the owner of its org.
pub async fn runtime_status(
    db: &DatabaseConnection,
    config: &Config,
    org_id: i64,
    flow_id: i64,
) -> FlowRuntimeStatus {
    let cfg = ClusterSettings::from_config(config);
    match owner_of(db, org_id).await {
        Some(owner) => transport::status(db, &cfg, &owner, flow_id)
            .await
            .unwrap_or_default(),
        None => FlowRuntimeStatus::default(),
    }
}

/// Debug feed (`limit` latest entries) or node statuses + last values
/// (`limit = None`) of one flow, asked to the owner of its org.
pub async fn debug_entries(
    db: &DatabaseConnection,
    config: &Config,
    org_id: i64,
    flow_id: i64,
    limit: Option<usize>,
) -> Vec<FlowDebugEntry> {
    let cfg = ClusterSettings::from_config(config);
    match owner_of(db, org_id).await {
        Some(owner) => transport::debug(db, &cfg, &owner, flow_id, limit)
            .await
            .unwrap_or_default(),
        None => Vec::new(),
    }
}

async fn owner_of(db: &DatabaseConnection, org_id: i64) -> Option<String> {
    store::placement(db, org_id)
        .await
        .ok()
        .flatten()
        .map(|p| p.worker_id)
}

// ───────────────────────────── Fencing ─────────────────────────────

type FenceCache = std::collections::HashMap<i64, (String, i64, std::time::Instant)>;

/// Is a runtime write for `org_id` stamped with `fence` (`<id>:<boot>`,
/// header [`pnex_core::FLOW_WORKER_HEADER`]) allowed? A runtime always
/// stamps its fence; a call without one comes from the backend itself (the
/// notify Test button goes through the same internal route) and is
/// allowed: the service token already authenticates, fencing is about
/// correctness, not access. Owner lookups are cached 1 s.
pub async fn fence_ok(db: &DatabaseConnection, org_id: i64, fence: Option<&str>) -> bool {
    static CACHE: std::sync::OnceLock<std::sync::Mutex<FenceCache>> = std::sync::OnceLock::new();
    let Some(raw) = fence else {
        return true;
    };
    let Some((id, boot)) = pnex_core::parse_flow_worker_fence(raw) else {
        return false;
    };
    let cache = CACHE.get_or_init(Default::default);
    let cached = cache
        .lock()
        .expect("fence cache")
        .get(&org_id)
        .filter(|(_, _, at)| at.elapsed() < Duration::from_secs(1))
        .map(|(w, b, _)| (w.clone(), *b));
    let owner = match cached {
        Some(o) => Some(o),
        None => {
            let Some(p) = store::placement(db, org_id).await.ok().flatten() else {
                return false;
            };
            let Some(w) = store::worker(db, &p.worker_id).await.ok().flatten() else {
                return false;
            };
            let mut c = cache.lock().expect("fence cache");
            if c.len() > 100_000 {
                c.clear();
            }
            c.insert(org_id, (w.id.clone(), w.boot, std::time::Instant::now()));
            Some((w.id, w.boot))
        }
    };
    owner.is_some_and(|(w, b)| w == id && b == boot)
}

/// Header value of the fencing identity, if any.
pub fn fence_header(headers: &axum::http::HeaderMap) -> Option<&str> {
    headers
        .get(pnex_core::FLOW_WORKER_HEADER)
        .and_then(|v| v.to_str().ok())
        // Nodes send an empty value outside a cluster worker.
        .filter(|v| !v.trim().is_empty())
}

/// Shared handle type of a local worker.
pub type WorkerHandle = Arc<WorkerNode>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn apply_error_round_trips_over_the_wire() {
        let cases = vec![
            ApplyError::NotOwner,
            ApplyError::Unreachable {
                error: "down".into(),
            },
            ApplyError::Check {
                checks: vec![FlowCheckError {
                    flow_id: 42,
                    message: "bad wire".into(),
                }],
            },
            ApplyError::Runtime {
                error: "boom".into(),
            },
        ];
        for c in cases {
            let json = serde_json::to_string(&c).unwrap();
            let back: ApplyError = serde_json::from_str(&json).unwrap();
            assert_eq!(format!("{back:?}"), format!("{c:?}"), "{json}");
        }
    }

    #[test]
    fn settings_keep_self_fencing_below_the_ttl() {
        let s = ClusterSettings {
            heartbeat_ms: 1_000,
            worker_ttl_ms: 1_000,
            self_fence_ms: 5_000,
            ..Default::default()
        }
        .sanitized();
        assert!(s.worker_ttl_ms >= 3 * s.heartbeat_ms);
        assert!(s.self_fence_ms < s.worker_ttl_ms);
        assert!(s.lease_ttl_ms >= 3 * s.controller_tick_ms);
    }
}
