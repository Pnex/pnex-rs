//! Flow worker (D106): one supervisor + the orgs placed on this worker.
//!
//! Tick (every `heartbeat_ms`):
//! 1. heartbeat `(id, boot)` — superseded ⇒ permanent fence; heartbeat
//!    failing for `self_fence_ms` ⇒ runtime killed until the database is
//!    reachable again (the controller replaces a worker only after
//!    `worker_ttl_ms` > `self_fence_ms`: the old runtime is down first);
//! 2. read its own placements (O(own orgs)) and reproject only the orgs
//!    added, moved or whose `revision` changed, plus every org at each
//!    full resync;
//! 3. hand the merged artifact to the supervisor (fire-and-forget: the
//!    runtime diffs tabs by hash, untouched tabs never restart).
//!
//! [`WorkerNode::apply_org`] (API push, deploy/stop acknowledgement) and
//! the tick are serialized by the state mutex, so a reconcile computed
//! from the database can never overwrite a pushed candidate mid-flight.
//! The pushed fragment does not update the `revision` seen: once the API
//! marks the database and bumps the revision, the tick reprojects the
//! same fragment (runtime no-op).

use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use sea_orm::DatabaseConnection;
use serde_json::Value;
use tokio::sync::Mutex;

use super::{store, transport, ApplyError, ApplyOrg, ClusterSettings};
use crate::services::flow::FlowSettings;
use crate::services::flow_supervisor::{DeployRequest, Supervisor};
use pnex_core::{FlowDebugEntry, FlowRuntimeStatus};

/// A boot number unique per process start (monotonic across restarts).
pub fn new_boot() -> i64 {
    static SEQ: std::sync::atomic::AtomicI64 = std::sync::atomic::AtomicI64::new(0);
    let ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0);
    // Two workers of one process booting in the same millisecond (tests)
    // still get distinct boots.
    ms * 1000 + SEQ.fetch_add(1, Ordering::SeqCst) % 1000
}

#[derive(Default)]
struct WorkerState {
    /// Projected entries per owned org.
    frags: BTreeMap<i64, Vec<Value>>,
    /// `(epoch, revision)` of each org as last projected from the database.
    seen: HashMap<i64, (i64, i64)>,
    last_full: Option<Instant>,
    /// Runtime killed by self-fencing (database unreachable).
    halted: bool,
}

pub struct WorkerNode {
    pub id: String,
    pub boot: i64,
    db: DatabaseConnection,
    cfg: ClusterSettings,
    supervisor: Supervisor,
    state: Mutex<WorkerState>,
    last_heartbeat_ok: std::sync::Mutex<Instant>,
    /// Permanently stopped (superseded, shut down, or killed in tests).
    stopped: AtomicBool,
    /// Last heartbeat succeeded (reconcile only runs while healthy).
    healthy: AtomicBool,
}

impl WorkerNode {
    /// Registers the worker, starts its supervisor and its tick loop.
    pub async fn start(
        db: DatabaseConnection,
        mut flow: FlowSettings,
        cfg: ClusterSettings,
    ) -> Result<Arc<Self>, sea_orm::DbErr> {
        let boot = new_boot();
        store::register_worker(&db, &cfg.worker_id, boot, &cfg.advertise_url, cfg.capacity).await?;
        flow.worker_fence = Some(format!("{}:{boot}", cfg.worker_id));
        let node = Arc::new(Self {
            id: cfg.worker_id.clone(),
            boot,
            db,
            supervisor: Supervisor::spawn(flow),
            cfg,
            state: Mutex::new(WorkerState::default()),
            last_heartbeat_ok: std::sync::Mutex::new(Instant::now()),
            stopped: AtomicBool::new(false),
            healthy: AtomicBool::new(true),
        });
        transport::register_local(node.clone());
        let period = Duration::from_millis(node.cfg.heartbeat_ms);
        // Heartbeat and reconcile run in separate tasks: a slow deploy
        // (holding the state lock while the runtime acknowledges) must
        // never delay the heartbeat, or a healthy worker would be failed
        // over.
        let weak = Arc::downgrade(&node);
        tokio::spawn(async move {
            loop {
                let Some(node) = weak.upgrade() else {
                    return;
                };
                if node.is_stopped() {
                    return;
                }
                node.heartbeat_once().await;
                drop(node);
                tokio::time::sleep(period).await;
            }
        });
        let weak = Arc::downgrade(&node);
        tokio::spawn(async move {
            // First round right away: flows placed on this id before a
            // restart resume without waiting for anything.
            loop {
                let Some(node) = weak.upgrade() else {
                    return;
                };
                if node.is_stopped() {
                    return;
                }
                if node.healthy.load(Ordering::SeqCst) {
                    if let Err(e) = node.reconcile().await {
                        tracing::warn!(worker = %node.id, error = %e, "flow worker reconcile failed (retried next round)");
                    }
                }
                drop(node);
                tokio::time::sleep(period).await;
            }
        });
        Ok(node)
    }

    pub fn fence(&self) -> String {
        format!("{}:{}", self.id, self.boot)
    }

    pub fn is_stopped(&self) -> bool {
        self.stopped.load(Ordering::SeqCst)
    }

    /// Flow ids currently in the artifact of this worker.
    pub async fn running_flows(&self) -> HashSet<i64> {
        let st = self.state.lock().await;
        st.frags
            .values()
            .flatten()
            .filter(|n| n.get("type").and_then(|t| t.as_str()) == Some("tab"))
            .filter_map(|n| n.get("pnex_flow_id").and_then(|v| v.as_i64()))
            .collect()
    }

    /// One heartbeat + reconcile round (public for deterministic tests).
    pub async fn tick(&self) {
        if self.heartbeat_once().await {
            if let Err(e) = self.reconcile().await {
                tracing::warn!(worker = %self.id, error = %e, "flow worker reconcile failed (retried next round)");
            }
        }
    }

    /// Heartbeat of `(id, boot)`; fences on supersession or on a silence
    /// longer than `self_fence_ms`. Returns whether the worker is healthy.
    async fn heartbeat_once(&self) -> bool {
        if self.is_stopped() {
            return false;
        }
        match store::heartbeat(&self.db, &self.id, self.boot).await {
            Ok(true) => {
                *self.last_heartbeat_ok.lock().expect("hb") = Instant::now();
                self.healthy.store(true, Ordering::SeqCst);
                true
            }
            Ok(false) => {
                tracing::error!(worker = %self.id, boot = self.boot, "flow worker superseded or collected — fencing");
                self.healthy.store(false, Ordering::SeqCst);
                self.stop_permanently().await;
                false
            }
            Err(e) => {
                self.healthy.store(false, Ordering::SeqCst);
                let silent = self.last_heartbeat_ok.lock().expect("hb").elapsed();
                tracing::warn!(worker = %self.id, error = %e, silent_ms = silent.as_millis() as u64, "flow worker heartbeat failed");
                if silent >= Duration::from_millis(self.cfg.self_fence_ms) {
                    self.self_fence().await;
                }
                false
            }
        }
    }

    async fn reconcile(&self) -> Result<(), String> {
        let mut st = self.state.lock().await;
        // Read under the lock: an apply that just recorded its placement
        // is never compared against an older read.
        let placements = store::placements_of(&self.db, &self.id)
            .await
            .map_err(|e| e.to_string())?;
        let full = st.halted
            || st
                .last_full
                .is_none_or(|t| t.elapsed() >= Duration::from_millis(self.cfg.full_resync_ms));
        let wanted: HashMap<i64, (i64, i64)> = placements
            .iter()
            .map(|p| (p.org_id, (p.epoch, p.revision)))
            .collect();
        let changed: Vec<i64> = wanted
            .iter()
            .filter(|(org, v)| full || st.seen.get(org) != Some(v))
            .map(|(org, _)| *org)
            .collect();
        let removed: Vec<i64> = st
            .frags
            .keys()
            .filter(|org| !wanted.contains_key(org))
            .copied()
            .collect();
        if changed.is_empty() && removed.is_empty() && !full {
            return Ok(());
        }
        let mut frags = st.frags.clone();
        for org in &removed {
            frags.remove(org);
        }
        for org in &changed {
            let fragment = crate::controllers::flows::project_org(&self.db, *org)
                .await
                .map_err(|e| format!("projection of org {org}: {e}"))?;
            match fragment {
                Value::Array(entries) if !entries.is_empty() => {
                    frags.insert(*org, entries);
                }
                _ => {
                    frags.remove(org);
                }
            }
        }
        let merged = merge(&frags);
        self.supervisor
            .deploy(DeployRequest {
                artifact: merged.clone(),
                ack: None,
                enforce_flow: None,
                // Every tab was checked when it was deployed; a tab broken
                // since is isolated by the runtime farm (flow_error).
                preflight_scope: Some(Value::Array(Vec::new())),
            })
            .await
            .map_err(|e| format!("{e:?}"))?;
        crate::services::camera::set_flow_demand_from(
            &self.id,
            crate::services::camera::flow_demand_of_artifact(&merged),
        );
        if !changed.is_empty() || !removed.is_empty() {
            tracing::info!(
                worker = %self.id,
                orgs = frags.len(),
                changed = changed.len(),
                removed = removed.len(),
                "flow worker reconciled"
            );
        }
        st.frags = frags;
        st.seen = wanted;
        st.halted = false;
        if full {
            st.last_full = Some(Instant::now());
        }
        Ok(())
    }

    /// Applies a pushed org fragment (deploy/stop of a flow, reprojection)
    /// and waits for the runtime acknowledgement.
    pub async fn apply_org(&self, req: ApplyOrg) -> Result<(), ApplyError> {
        if self.is_stopped() {
            return Err(ApplyError::NotOwner);
        }
        let mut st = self.state.lock().await;
        // Ownership is checked in the database under the state lock: the
        // controller may have moved the org since the caller resolved it.
        let placement = store::placement(&self.db, req.org_id)
            .await
            .map_err(|e| ApplyError::Runtime {
                error: e.to_string(),
            })?
            .filter(|p| p.worker_id == self.id);
        let Some(placement) = placement.filter(|_| !st.halted) else {
            return Err(ApplyError::NotOwner);
        };
        let entries = match &req.fragment {
            Value::Array(e) => e.clone(),
            _ => Vec::new(),
        };
        let mut frags = st.frags.clone();
        if entries.is_empty() {
            frags.remove(&req.org_id);
        } else {
            frags.insert(req.org_id, entries);
        }
        let merged = merge(&frags);
        self.supervisor
            .deploy(DeployRequest {
                artifact: merged.clone(),
                ack: req.ack,
                enforce_flow: req.enforce_flow,
                preflight_scope: Some(req.fragment),
            })
            .await?;
        crate::services::camera::set_flow_demand_from(
            &self.id,
            crate::services::camera::flow_demand_of_artifact(&merged),
        );
        st.frags = frags;
        // The pushed fragment is this placement's truth until the API marks
        // the database and bumps the revision: a tick must not reproject
        // the org from a database that does not reflect the push yet.
        st.seen
            .insert(req.org_id, (placement.epoch, placement.revision));
        Ok(())
    }

    pub fn runtime_status(&self, flow_id: i64) -> FlowRuntimeStatus {
        self.supervisor.runtime_status(flow_id)
    }

    /// Debug feed (`Some(limit)`) or node statuses + last values (`None`).
    pub fn debug_entries(&self, flow_id: i64, limit: Option<usize>) -> Vec<FlowDebugEntry> {
        use crate::services::flow_supervisor as sup;
        match limit {
            Some(n) => sup::debug_entries(flow_id, n),
            None => {
                let mut e = sup::node_statuses(flow_id);
                e.extend(sup::node_last_values(flow_id));
                e
            }
        }
    }

    /// Runtime down while the database is unreachable (lease lost).
    async fn self_fence(&self) {
        let mut st = self.state.lock().await;
        if st.halted {
            return;
        }
        tracing::error!(worker = %self.id, "flow worker lost its heartbeat — runtime halted (self-fencing)");
        self.supervisor.halt().await;
        crate::services::camera::set_flow_demand_from(&self.id, HashSet::new());
        st.frags.clear();
        st.seen.clear();
        st.halted = true;
    }

    async fn stop_permanently(&self) {
        if self.stopped.swap(true, Ordering::SeqCst) {
            return;
        }
        let mut st = self.state.lock().await;
        self.supervisor.halt().await;
        crate::services::camera::set_flow_demand_from(&self.id, HashSet::new());
        st.frags.clear();
        st.seen.clear();
        transport::deregister_local(&self.id, self.boot);
    }

    /// Graceful stop: with `drain`, mark the worker draining and wait
    /// (within `drain_timeout_ms`) for the controller to hand its orgs to
    /// another live worker, then halt the runtime and deregister.
    pub async fn shutdown(&self, drain: bool) {
        if self.is_stopped() {
            return;
        }
        if drain {
            let _ = store::set_draining(&self.db, &self.id, self.boot).await;
            let deadline = Instant::now() + Duration::from_millis(self.cfg.drain_timeout_ms);
            while Instant::now() < deadline {
                let others = store::workers(&self.db)
                    .await
                    .unwrap_or_default()
                    .into_iter()
                    .any(|w| {
                        w.id != self.id
                            && !w.draining
                            && store::is_alive(&w, self.cfg.worker_ttl_ms)
                    });
                let mine = store::placements_of(&self.db, &self.id)
                    .await
                    .map(|p| p.len())
                    .unwrap_or(0);
                if mine == 0 || !others {
                    break;
                }
                // Keep reconciling: released orgs leave the runtime.
                self.tick().await;
                tokio::time::sleep(Duration::from_millis(self.cfg.heartbeat_ms)).await;
            }
        }
        self.stop_permanently().await;
        let _ = store::deregister_worker(&self.db, &self.id, self.boot).await;
        tracing::info!(worker = %self.id, "flow worker stopped");
    }

    /// Test hook: the process "dies" — no heartbeat, no deregistration,
    /// runtime killed. The controller must fail its orgs over.
    pub async fn crash_for_tests(&self) {
        self.stop_permanently().await;
    }
}

/// Merged artifact: every owned org, in org order (deterministic hash).
fn merge(frags: &BTreeMap<i64, Vec<Value>>) -> Value {
    Value::Array(frags.values().flatten().cloned().collect())
}
