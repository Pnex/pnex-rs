//! Per-flow engine health and per-flow deploy acknowledgements.

use super::*;

// ──────────────── Santé par flow (ferme « N engines, 1 process ») ────────────────

/// État de l'engine d'un flow, dérivé des événements stdout du runtime.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FlowEngineHealth {
    /// `flow_started` — engine chargé et démarré (ou confirmation idempotente).
    Running,
    /// `flow_error` — build/démarrage en échec. Si un last-good tournait, il
    /// continue d'ingérer (nuance : le chip peut montrer « error » avec une
    /// ingestion qui continue sur l'ancienne version).
    Error,
    /// `flow_stopped` — tab disparu de l'artefact (dé-déploiement, retrait).
    Stopped,
}

impl FlowEngineHealth {
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

/// Santé par flow : alimentée par les événements `flow_*` du stdout du
/// runtime (le runtime ré-annonce tous les tabs au boot d'un enfant — un
/// respawn self-heals la carte). Mémoire du process backend uniquement :
/// pas de version ici non plus, la vérité « version déployée » reste la DB.
static FLOW_HEALTH: OnceLock<std::sync::Mutex<std::collections::HashMap<i64, FlowHealthEntry>>> =
    OnceLock::new();

fn health() -> &'static std::sync::Mutex<std::collections::HashMap<i64, FlowHealthEntry>> {
    FLOW_HEALTH.get_or_init(std::sync::Mutex::default)
}

/// Sonde de test/utilitaire : état courant d'un flow (None = inconnu).
pub fn flow_engine_health(flow_id: i64) -> Option<(FlowEngineHealth, Option<String>)> {
    health()
        .lock()
        .expect("lock flow health")
        .get(&flow_id)
        .map(|e| (e.status, e.last_error.clone()))
}

/// Sonde de test : vide la carte de santé (isolation entre tests).
pub fn reset_flow_health_for_tests() {
    health().lock().expect("lock flow health").clear();
}

/// Ingère un événement `flow_started`/`flow_error`/`flow_stopped` :
/// met à jour la santé par flow.
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
        "santé engine de flow mise à jour"
    );
}

// ─────────────── Acquittement deploy par flow (oneshot par flow) ───────────────

/// Dépile l'acquittement d'un deploy sur `flow_started`/`flow_error`/
/// `flow_stopped` (retrait du tab : le changement a été appliqué — c'est
/// l'ack des dé-déploiements).
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
