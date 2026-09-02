//! Services métier (hors contrôleleurs HTTP) : télémétrie, bail de vie,
//! OpenObserve, dashboard, réglages firmware.

pub mod artifact_store;
pub mod dashboard;
pub mod device_liveness;
pub mod firmware;
pub mod openobserve;
pub mod settings;
pub mod provisioning;
pub mod telemetry;
pub mod visualization;
