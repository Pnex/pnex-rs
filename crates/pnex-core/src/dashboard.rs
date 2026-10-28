//! Dashboard DTO (2026-08-19) — single response of
//! `GET /api/v1/dashboard/summary`: PG liveness + latest OpenObserve
//! measurements + build stats.
//!
//! Telemetry lives only in OpenObserve (D1, no measurement
//! table in PG): its section is therefore **degraded by design** —
//! `telemetry.available == false` when the org has no O2 credentials or
//! the query fails/times out. The PG sections (liveness, builds) are
//! always served: the summary never returns 500 from the O2 branch.
//!
//! Date fields as RFC 3339 strings (SeaORM serialization / Prometheus epoch
//! timestamps converted on the backend), no chrono in the core
//! (wasm32).

use serde::{Deserialize, Serialize};

/// Answer of `GET /api/v1/dashboard/summary`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct DashboardSummary {
    pub liveness: LivenessSummary,
    pub telemetry: TelemetrySummary,
    pub builds: BuildStats,
}

/// Devices of the org with their live state (join of
/// `device_registries` and `device_states`).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct LivenessSummary {
    /// Full org counts: the list is truncated, the counters are not.
    pub total: u64,
    /// Devices whose `last_seen_at` is fresher than the silence TTL.
    pub live: u64,
    /// Live devices first, then by `last_seen` descending; capped at ~10
    /// by the backend.
    pub devices: Vec<DeviceLiveness>,
    /// Device count per device type over the whole org (not capped).
    #[serde(default)]
    pub by_type: std::collections::BTreeMap<String, u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct DeviceLiveness {
    pub id: i64,
    /// Business `device_id` declared by the firmware (the numeric PK is `id`).
    pub device_id: String,
    /// Predefined model (`generic_esp8266`, ...).
    pub predefined_device_name: String,
    pub device_type: String,
    pub live: bool,
    /// RFC 3339, `null` = never seen.
    pub last_seen: Option<String>,
}

/// Latest measurements; degraded when OpenObserve is unavailable.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct TelemetrySummary {
    /// False: no OpenObserve credentials for the org, error or timeout
    /// (3 s). The UI shows a notice, never a blocking error.
    pub available: bool,
    /// Latest sample per (metric, device), newest first, capped at ~10
    /// rows by the backend.
    pub latest: Vec<LatestMeasurement>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct LatestMeasurement {
    /// Prometheus `__name__` label (e.g. `soil_moisture`).
    pub metric: String,
    pub device_id: String,
    pub value: f64,
    /// RFC 3339; `null` when the sample carried none.
    pub timestamp: Option<String>,
}

/// Aggregate of the org `build_records` (bounded: one per device).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct BuildStats {
    pub total: u64,
    pub succeeded: u64,
    /// `succeeded / total` in [0,1]; 0.0 when total is 0 (never NaN).
    pub success_rate: f64,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Full form as serialized by the backend.
    #[test]
    fn dashboard_summary_roundtrip() {
        let json = r#"{
            "liveness": {
                "total": 2,
                "live": 1,
                "devices": [
                    {
                        "id": 11,
                        "device_id": "capteur-jardin",
                        "predefined_device_name": "temp_sensor",
                        "device_type": "sensor",
                        "live": true,
                        "last_seen": "2026-08-19T10:00:30+00:00"
                    },
                    {
                        "id": 12,
                        "device_id": "relais-pompe",
                        "predefined_device_name": "relay_1ch",
                        "device_type": "actuator",
                        "live": false,
                        "last_seen": null
                    }
                ],
                "by_type": { "actuator": 1, "sensor": 1 }
            },
            "telemetry": {
                "available": true,
                "latest": [
                    {
                        "metric": "soil_moisture",
                        "device_id": "capteur-jardin",
                        "value": 42.5,
                        "timestamp": "2026-08-19T10:00:30+00:00"
                    }
                ]
            },
            "builds": { "total": 3, "succeeded": 2, "success_rate": 0.6666666666666666 }
        }"#;
        let summary: DashboardSummary = serde_json::from_str(json).unwrap();
        assert_eq!(summary.liveness.total, 2);
        assert_eq!(summary.liveness.live, 1);
        assert!(summary.liveness.devices[0].live);
        assert_eq!(summary.liveness.by_type["sensor"], 1);
        assert!(summary.telemetry.available);
        assert_eq!(summary.telemetry.latest[0].metric, "soil_moisture");
        assert_eq!(summary.builds.total, 3);
        let back = serde_json::to_value(&summary).unwrap();
        assert_eq!(
            back,
            serde_json::from_str::<serde_json::Value>(json).unwrap()
        );
    }

    /// Empty org / O2 absent: everything at zero, available false — the dashboard
    /// page must be able to draw itself with this form alone.
    #[test]
    fn dashboard_summary_minimal_degrade() {
        let json = r#"{
            "liveness": { "total": 0, "live": 0, "devices": [] },
            "telemetry": { "available": false, "latest": [] },
            "builds": { "total": 0, "succeeded": 0, "success_rate": 0.0 }
        }"#;
        let summary: DashboardSummary = serde_json::from_str(json).unwrap();
        assert!(summary.liveness.devices.is_empty());
        assert!(!summary.telemetry.available);
        assert_eq!(summary.builds.success_rate, 0.0);
    }
}
