//! Point de télémétrie + sink remplaçable.
//!
//! Le WS d'ingestion pousse ses points dans le sink global sans jamais
//! bloquer : l'implémentation réelle (batcher OpenObserve, Phase 5) bufferise
//! et flushe par lots ; les tests injectent un sink enregistreur. Forme du
//! point = the unified document shape of the legacy ES pipeline,
//! org-scoped (D2) — `user_id` became `org_id`.

use std::sync::{Arc, RwLock};

/// Un point de mesure ingéré (→ stream OpenObserve `sensor_measurements`).
#[derive(Debug, Clone)]
pub struct TelemetryPoint {
    pub org_id: i64,
    pub device_registry_id: i64,
    /// Identifiant déclaré par le firmware (MAC/hostname).
    pub device_id: String,
    /// Nom du predefined device (dimension `pred_dev`).
    pub pred_dev: String,
    pub metric_name: String,
    /// Raw text value — cast to float at the edge (OpenObserve) like the
    /// legacy ES consumer used to do.
    pub value: String,
    /// Horodatage serveur à la réception (v1 ; D12 : `ts_source` prêt pour
    /// un timestamp device en protocole v2).
    pub timestamp: chrono::DateTime<chrono::Utc>,
    pub ts_source: &'static str,
    pub source_type: &'static str,
    /// Record in OpenObserve (history). `false` = live-only: the point still
    /// reaches the Valkey last-value cache (flows, memory, live dashboards)
    /// but never the O2 batcher — edge agent keys whose `record_o2` toggle
    /// is off (D95). Every other producer records.
    pub record: bool,
}

/// Réceptacle des points ingérés — ne doit jamais bloquer la boucle WS.
pub trait TelemetrySink: Send + Sync {
    fn send(&self, point: TelemetryPoint);
}

/// Sink par défaut : abandonne les points (avant branchement OpenObserve).
struct NoopSink;

impl TelemetrySink for NoopSink {
    fn send(&self, _point: TelemetryPoint) {}
}

static SINK: RwLock<Option<Arc<dyn TelemetrySink>>> = RwLock::new(None);

/// Sink actif (NoopSink tant que non installé).
pub fn sink() -> Arc<dyn TelemetrySink> {
    SINK.read()
        .expect("verrou sink")
        .clone()
        .unwrap_or_else(|| Arc::new(NoopSink))
}

/// Installe le sink (boot : batcher OpenObserve ; tests : sink enregistreur).
pub fn set_sink(new: Arc<dyn TelemetrySink>) {
    *SINK.write().expect("verrou sink") = Some(new);
}

/// Retire le sink installé (restaure NoopSink) — hygiène de tests.
pub fn reset_sink() {
    *SINK.write().expect("verrou sink") = None;
}

/// Default sink as an `Arc` — lets the boot composition install a real chain
/// (Valkey tap over noop) even when OpenObserve is unconfigured.
pub fn noop_sink() -> Arc<dyn TelemetrySink> {
    Arc::new(NoopSink)
}
