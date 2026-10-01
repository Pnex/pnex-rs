//! Worker transport (D106): in-process when the owner lives in this
//! process (embedded mode, tests), internal HTTP otherwise
//! (`controllers::flow_cluster`, header `x-pnex-cluster-token`).

use std::sync::{Arc, LazyLock, Mutex};
use std::time::Duration;

use sea_orm::DatabaseConnection;

use super::{store, ApplyError, ApplyOrg, ClusterSettings, WorkerNode};
use pnex_core::{FlowDebugEntry, FlowRuntimeStatus};

/// Header of the internal cluster routes.
pub const CLUSTER_TOKEN_HEADER: &str = "x-pnex-cluster-token";
/// Target worker of an internal call (the receiving process routes it to
/// that local worker, never to "any" worker it hosts).
pub const CLUSTER_WORKER_HEADER: &str = "x-pnex-cluster-worker";

/// Workers reached over HTTP even when they live in this process (test
/// hook: exercises the real pod-to-pod path inside one test process).
static FORCE_REMOTE: LazyLock<Mutex<std::collections::HashSet<String>>> =
    LazyLock::new(|| Mutex::new(std::collections::HashSet::new()));

/// Test hook: calls to worker `id` go over HTTP (its `advertise_url`).
pub fn force_remote_for_tests(id: &str) {
    FORCE_REMOTE
        .lock()
        .expect("force remote")
        .insert(id.to_string());
}

/// The local worker `id` for the in-process shortcut (`None` when forced
/// remote).
fn shortcut(id: &str) -> Option<Arc<WorkerNode>> {
    if FORCE_REMOTE.lock().expect("force remote").contains(id) {
        return None;
    }
    local(id)
}

static LOCAL: LazyLock<Mutex<Vec<Arc<WorkerNode>>>> = LazyLock::new(|| Mutex::new(Vec::new()));

pub(super) fn register_local(node: Arc<WorkerNode>) {
    let mut l = LOCAL.lock().expect("local workers");
    l.retain(|n| n.id != node.id);
    l.push(node);
}

pub(super) fn deregister_local(id: &str, boot: i64) {
    LOCAL
        .lock()
        .expect("local workers")
        .retain(|n| !(n.id == id && n.boot == boot));
}

/// Live workers of this process.
pub fn local_workers() -> Vec<Arc<WorkerNode>> {
    LOCAL.lock().expect("local workers").clone()
}

/// The in-process worker `id`, if it lives here.
pub fn local(id: &str) -> Option<Arc<WorkerNode>> {
    LOCAL
        .lock()
        .expect("local workers")
        .iter()
        .find(|n| n.id == id && !n.is_stopped())
        .cloned()
}

fn client(cfg: &ClusterSettings) -> reqwest::Client {
    static CLIENT: std::sync::OnceLock<reqwest::Client> = std::sync::OnceLock::new();
    let timeout = Duration::from_millis(cfg.request_timeout_ms);
    CLIENT
        .get_or_init(|| {
            reqwest::Client::builder()
                .connect_timeout(Duration::from_secs(3))
                .timeout(timeout)
                .build()
                .expect("cluster http client")
        })
        .clone()
}

async fn remote_url(db: &DatabaseConnection, owner: &str) -> Result<String, ApplyError> {
    let w = store::worker(db, owner)
        .await
        .map_err(|e| ApplyError::Unreachable {
            error: e.to_string(),
        })?
        .ok_or_else(|| ApplyError::Unreachable {
            error: "unknown worker".into(),
        })?;
    if w.advertise_url.is_empty() {
        return Err(ApplyError::Unreachable {
            error: "worker has no advertise_url (embedded mode on another process?)".into(),
        });
    }
    Ok(w.advertise_url)
}

pub(super) async fn apply(
    db: &DatabaseConnection,
    cfg: &ClusterSettings,
    owner: &str,
    req: &ApplyOrg,
) -> Result<(), ApplyError> {
    if let Some(node) = shortcut(owner) {
        return node.apply_org(req.clone()).await;
    }
    let url = remote_url(db, owner).await?;
    let res = client(cfg)
        .post(format!("{url}/internal/flow-cluster/apply"))
        .header(CLUSTER_TOKEN_HEADER, &cfg.token)
        .header(CLUSTER_WORKER_HEADER, owner)
        .json(req)
        .send()
        .await
        .map_err(|e| ApplyError::Unreachable {
            error: e.to_string(),
        })?;
    let status = res.status();
    if status.is_success() {
        return Ok(());
    }
    // The worker answers an `ApplyError` body on every handled failure.
    match res.json::<ApplyError>().await {
        Ok(e) => Err(e),
        Err(_) => Err(ApplyError::Unreachable {
            error: format!("HTTP {status}"),
        }),
    }
}

pub(super) async fn status(
    db: &DatabaseConnection,
    cfg: &ClusterSettings,
    owner: &str,
    flow_id: i64,
) -> Option<FlowRuntimeStatus> {
    if let Some(node) = shortcut(owner) {
        return Some(node.runtime_status(flow_id));
    }
    let url = remote_url(db, owner).await.ok()?;
    client(cfg)
        .get(format!(
            "{url}/internal/flow-cluster/flows/{flow_id}/runtime"
        ))
        .header(CLUSTER_TOKEN_HEADER, &cfg.token)
        .header(CLUSTER_WORKER_HEADER, owner)
        .send()
        .await
        .ok()?
        .error_for_status()
        .ok()?
        .json()
        .await
        .ok()
}

pub(super) async fn debug(
    db: &DatabaseConnection,
    cfg: &ClusterSettings,
    owner: &str,
    flow_id: i64,
    limit: Option<usize>,
) -> Option<Vec<FlowDebugEntry>> {
    if let Some(node) = shortcut(owner) {
        return Some(node.debug_entries(flow_id, limit));
    }
    let url = remote_url(db, owner).await.ok()?;
    let mut req = client(cfg)
        .get(format!("{url}/internal/flow-cluster/flows/{flow_id}/debug"))
        .header(CLUSTER_TOKEN_HEADER, &cfg.token)
        .header(CLUSTER_WORKER_HEADER, owner);
    if let Some(n) = limit {
        req = req.query(&[("limit", n)]);
    }
    req.send()
        .await
        .ok()?
        .error_for_status()
        .ok()?
        .json()
        .await
        .ok()
}
