//! Services métier (hors contrôleleurs HTTP) : télémétrie, bail de vie,
//! OpenObserve, dashboard, réglages firmware, moteur de flow ETL (D18).

pub mod artifact_store;
pub mod dashboard;
pub mod device_liveness;
pub mod firmware;
pub mod flow;
pub mod flow_supervisor;
pub mod openobserve;
pub mod settings;
pub mod provisioning;
pub mod telemetry;
pub mod visualization;
