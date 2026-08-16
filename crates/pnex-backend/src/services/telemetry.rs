//! Point de télémétrie + sink remplaçable.
//!
//! Le WS d'ingestion pousse ses points dans le sink global sans jamais
//! bloquer : l'implémentation réelle (batcher OpenObserve, Phase 5) bufferise
//! et flushe par lots ; les tests injectent un sink enregistreur. Forme du
//! point = document unifié Django/ES (`docs/phase0/etl-es-metrics.md` §5),
//! scopé org (D2) — `user_id` Django devient `org_id`.

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
    /// Valeur brute texte — flottée au bord (OpenObserve) comme le faisait
    /// le consumer ES Django.
    pub value: String,
    /// Horodatage serveur à la réception (v1 ; D12 : `ts_source` prêt pour
    /// un timestamp device en protocole v2).
    pub timestamp: chrono::DateTime<chrono::Utc>,
    pub ts_source: &'static str,
    pub source_type: &'static str,
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
