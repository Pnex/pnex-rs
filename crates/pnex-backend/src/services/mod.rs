//! Services métier (hors contrôleleurs HTTP) : télémétrie, bail de vie,
//! OpenObserve, réglages firmware.

pub mod device_liveness;
pub mod firmware;
pub mod openobserve;
pub mod settings;
pub mod telemetry;
