//! Per-flow engine health and per-flow deploy acknowledgements.

use super::*;

// ──────────────── Per-flow health (closes « N engines, 1 process ») ────────────────

/// State of a flow's engine, derived from the runtime's stdout events.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FlowEngineHealth {
    /// `flow_started` — engine loaded and started (or idempotent confirmation).
    Running,
    /// `flow_error` — build/start failed. If a last-good was running, it
    /// keeps ingesting (nuance: the chip may show "error" while
    /// ingestion continues on the old version).
    Error,
    /// `flow_stopped` — tab gone from the artifact (undeploy, removal).
    Stopped,
}

impl FlowEngineHealth {
    /// Wire value of the runtime status DTO.
    pub fn dto(&self) -> pnex_core::FlowEngineStatus {
        match self {
            Self::Running => pnex_core::FlowEngineStatus::Running,
            Self::Error => pnex_core::FlowEngineStatus::Error,
            Self::Stopped => pnex_core::FlowEngineStatus::Stopped,
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Running => "running",
            Self::Error => "error",
            Self::Stopped => "stopped",
        }
    }
}

#[derive(Debug, Clone)]
struct FlowHealthEntry {
    status: FlowEngineHealth,
    last_error: Option<String>,
}

/// Per-flow health: fed by the `flow_*` events of the runtime's stdout
/// (the runtime re-announces all tabs when a child boots — a
/// respawn self-heals the map). Backend process memory only:
/// no version here either, the "deployed version" truth stays in the DB.
static FLOW_HEALTH: OnceLock<std::sync::Mutex<std::collections::HashMap<i64, FlowHealthEntry>>> =
    OnceLock::new();

fn health() -> &'static std::sync::Mutex<std::collections::HashMap<i64, FlowHealthEntry>> {
    FLOW_HEALTH.get_or_init(std::sync::Mutex::default)
}

/// Test/utility probe: current state of a flow (None = unknown).
pub fn flow_engine_health(flow_id: i64) -> Option<(FlowEngineHealth, Option<String>)> {
    health()
        .lock()
        .expect("lock flow health")
        .get(&flow_id)
        .map(|e| (e.status, e.last_error.clone()))
}

/// Test probe: clears the health map (isolation between tests).
pub fn reset_flow_health_for_tests() {
    health().lock().expect("lock flow health").clear();
}

/// Ingests a `flow_started`/`flow_error`/`flow_stopped` event:
/// updates the per-flow health.
pub(super) fn record_flow_health(v: &Value) {
    let Some(fid) = v.get("flow").and_then(|f| f.as_i64()) else {
        return;
    };
    let event = v.get("event").and_then(|e| e.as_str()).unwrap_or_default();
    let (status, last_error) = match event {
        "flow_started" => (FlowEngineHealth::Running, None),
        "flow_error" => (
            FlowEngineHealth::Error,
            Some(
                v.get("error")
                    .and_then(|e| e.as_str())
                    .unwrap_or("unknown runtime error")
                    .to_string(),
            ),
        ),
        _ => (FlowEngineHealth::Stopped, None),
    };
    health()
        .lock()
        .expect("lock flow health")
        .insert(fid, FlowHealthEntry { status, last_error });
    tracing::debug!(
        flow_id = fid,
        status = status.as_str(),
        "flow engine health updated"
    );
}

// ─────────────── Per-flow deploy acknowledgement (oneshot per flow) ───────────────

/// Pops a deploy's acknowledgement on `flow_started`/`flow_error`/
/// `flow_stopped` (tab removal: the change was applied — this is
/// the ack for undeploys).
pub(super) fn resolve_deploy_ack(v: &Value, pending: &PendingAcks) {
    let Some(fid) = v.get("flow").and_then(|f| f.as_i64()) else {
        return;
    };
    let mut pending = pending.lock().expect("lock pending deploy");
    if let Some(tx) = pending.remove(&fid) {
        match v.get("event").and_then(|e| e.as_str()) {
            Some("flow_started" | "flow_stopped") => {
                let _ = tx.send(Ok(()));
            }
            _ => {
                let err = v
                    .get("error")
                    .and_then(|e| e.as_str())
                    .unwrap_or("flow rejected by the runtime")
                    .to_string();
                let _ = tx.send(Err(err));
            }
        }
    }
}
