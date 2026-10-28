//! Telemetry point + replaceable sink.
//!
//! The ingestion WS pushes its points into the global sink without ever
//! blocking: the real implementation (OpenObserve batcher, Phase 5) buffers
//! and flushes in batches; tests inject a recording sink.
//! Points are org-scoped (D2).

use std::sync::{Arc, RwLock};

/// An ingested measurement point (→ OpenObserve stream `sensor_measurements`).
#[derive(Debug, Clone)]
pub struct TelemetryPoint {
    pub org_id: i64,
    pub device_registry_id: i64,
    /// Identifier declared by the firmware (MAC/hostname).
    pub device_id: String,
    /// Name of the predefined device (`pred_dev` dimension).
    pub pred_dev: String,
    pub metric_name: String,
    /// Raw text value — cast to float at the edge (OpenObserve).
    pub value: String,
    /// Server timestamp at reception (v1; D12: `ts_source` ready for a
    /// device timestamp in protocol v2).
    pub timestamp: chrono::DateTime<chrono::Utc>,
    pub ts_source: &'static str,
    pub source_type: &'static str,
    /// Record in OpenObserve (history). `false` = live-only: the point still
    /// reaches the Valkey last-value cache (flows, memory, live dashboards)
    /// but never the O2 batcher — edge agent keys whose `record_o2` toggle
    /// is off (D95). Every other producer records.
    pub record: bool,
}

/// Receiver of ingested points — must never block the WS loop.
pub trait TelemetrySink: Send + Sync {
    fn send(&self, point: TelemetryPoint);
}

/// Default sink: drops the points (before OpenObserve is wired).
struct NoopSink;

impl TelemetrySink for NoopSink {
    fn send(&self, _point: TelemetryPoint) {}
}

static SINK: RwLock<Option<Arc<dyn TelemetrySink>>> = RwLock::new(None);

/// Active sink (NoopSink until one is installed).
pub fn sink() -> Arc<dyn TelemetrySink> {
    SINK.read()
        .expect("sink lock")
        .clone()
        .unwrap_or_else(|| Arc::new(NoopSink))
}

/// Installs the sink (boot: OpenObserve batcher; tests: recording sink).
pub fn set_sink(new: Arc<dyn TelemetrySink>) {
    *SINK.write().expect("sink lock") = Some(new);
}

/// Removes the installed sink (restores NoopSink) — test hygiene.
pub fn reset_sink() {
    *SINK.write().expect("sink lock") = None;
}

/// Default sink as an `Arc` — lets the boot composition install a real chain
/// (Valkey tap over noop) even when OpenObserve is unconfigured.
pub fn noop_sink() -> Arc<dyn TelemetrySink> {
    Arc::new(NoopSink)
}
