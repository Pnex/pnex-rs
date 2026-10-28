//! Telemetry DTOs (2026-08-19) — Visualization page: time
//! series read from OpenObserve (Prometheus) via
//! `GET /api/v1/telemetry/catalog` and `GET /api/v1/telemetry/series`.
//!
//! Same degraded doctrine as the dashboard: the O2 read never makes
//! the request fail — without O2 credentials, on error or timeout, the
//! payload returns `available == false` and empty lists.
//!
//! Date fields: timestamps in **epoch seconds** as f64 (native
//! Prometheus form, directly usable by the front's SVG chart);
//! catalog `last_seen` in RFC 3339 (converted on the backend). No
//! chrono in the core (wasm32).

use serde::{Deserialize, Serialize};

/// Response of `GET /api/v1/telemetry/catalog` — available series
/// (metric × device) to feed the page's selectors.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[cfg_attr(
    feature = "openapi",
    schema(
        description = "Available series (metric x device) and label sets, for the pickers. Over HTTP, OpenObserve trouble answers 503 `telemetry-unavailable`, so `available` is always true there."
    )
)]
pub struct TelemetryCatalog {
    /// False: no OpenObserve credentials for the org, error or timeout (5 s).
    pub available: bool,
    pub series: Vec<TelemetrySeriesInfo>,
    /// Series without a device, read by label set (D171): platform
    /// `media_*` series and labelled `etl_` series. Bounded per metric.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub label_sets: Vec<TelemetryLabelSet>,
}

/// One label set of a metric offered to the dashboard pickers (D171).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct TelemetryLabelSet {
    pub metric: String,
    pub labels: std::collections::BTreeMap<String, String>,
}

/// Key of a series in the live-values map: `metric|device_id`, plus the
/// labels in their sorted order when there are any (D171).
pub fn series_key(
    metric: &str,
    device_id: &str,
    labels: &std::collections::BTreeMap<String, String>,
) -> String {
    let mut key = format!("{metric}|{device_id}");
    if !labels.is_empty() {
        let pairs: Vec<String> = labels.iter().map(|(k, v)| format!("{k}={v}")).collect();
        key.push('|');
        key.push_str(&pairs.join(","));
    }
    key
}

/// An available series = a metric on a device (latest value
/// over 24 h — also shows that the data is alive).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[cfg_attr(
    feature = "openapi",
    schema(
        description = "An available series: one metric on one device, with its last value over 24 h."
    )
)]
pub struct TelemetrySeriesInfo {
    /// Prometheus `__name__` label (e.g. `soil_moisture`).
    pub metric: String,
    pub device_id: String,
    /// Predefined model carried at ingest (`pred_dev`), if any.
    pub pred_dev: Option<String>,
    pub last_value: f64,
    /// RFC 3339 of the last sample, `None` when it carried none.
    #[cfg_attr(feature = "openapi", schema(format = DateTime))]
    pub last_seen: Option<String>,
}

/// Response of `GET /api/v1/telemetry/series?metric=…&device_id=…&window=…`
/// — points of ONE series over the requested window.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[cfg_attr(
    feature = "openapi",
    schema(
        description = "Points of one series over the requested window. `available: false` = OpenObserve unavailable (`points` empty); only batch items carry it, `GET /api/v1/telemetry/series` answers 503 `telemetry-unavailable` instead."
    )
)]
pub struct TelemetrySeriesResponse {
    /// False: missing credentials, OpenObserve error or timeout; `points` empty.
    pub available: bool,
    pub metric: String,
    pub device_id: String,
    /// Sorted by ascending `ts`, capped by the backend.
    pub points: Vec<TelemetryPoint>,
}

/// A point of the series — epoch seconds (f64, Prometheus form: the
/// fractional part is possible), numeric value already re-parsed.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[cfg_attr(
    feature = "openapi",
    schema(description = "A point: `ts` epoch seconds (may be fractional), numeric value.")
)]
pub struct TelemetryPoint {
    pub ts: f64,
    pub value: f64,
}

// ───────────────────────── Batch (studio SCADA, D31) ─────────────────────────

/// A series reference of `POST /api/v1/telemetry/series/query` — the
/// same triple as the widget sources (`viz::SourceRef`): a single
/// call for a whole dashboard (one timer instead of N, D31).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[cfg_attr(
    feature = "openapi",
    schema(
        description = "A series reference of the batch: metric, device and/or labels, window preset."
    )
)]
pub struct SeriesSpec {
    pub metric: String,
    pub device_id: String,
    /// Window preset (`viz::VIZ_WINDOW_PRESETS`).
    pub window: String,
    /// Free label selector (D171); with labels, `device_id` may be empty
    /// (the read then sums the matching series).
    #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub labels: std::collections::BTreeMap<String, String>,
}

impl SeriesSpec {
    /// Key of this series in the live-values map (same as `SourceRef`).
    pub fn series_key(&self) -> String {
        series_key(&self.metric, &self.device_id, &self.labels)
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct SeriesBatchRequest {
    pub specs: Vec<SeriesSpec>,
}

/// Batch response — one item per spec, **same order**; an invalid
/// or unserved spec (budget exceeded) is **degraded** at the item
/// level (`available: false`), never a global 400.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[cfg_attr(
    feature = "openapi",
    schema(
        description = "One item per spec, same order; an invalid or unserved spec is degraded per item (`available: false`), never a global error."
    )
)]
pub struct SeriesBatchResponse {
    pub available: bool,
    pub results: Vec<TelemetrySeriesResponse>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn labels_change_the_series_key() {
        let mut labels = std::collections::BTreeMap::new();
        assert_eq!(series_key("m", "d", &labels), "m|d");
        labels.insert("b".to_string(), "2".to_string());
        labels.insert("a".to_string(), "1".to_string());
        assert_eq!(series_key("m", "", &labels), "m||a=1,b=2");
    }

    /// Full form as serialized by the backend.
    #[test]
    fn telemetry_roundtrip() {
        let json = r#"{
            "available": true,
            "series": [
                {
                    "metric": "soil_moisture",
                    "device_id": "fuzzy-zebra",
                    "pred_dev": "temp_sensor",
                    "last_value": 100.0,
                    "last_seen": "2026-08-19T10:00:30+00:00"
                },
                {
                    "metric": "soil_temperature",
                    "device_id": "fuzzy-zebra",
                    "pred_dev": null,
                    "last_value": 21.5,
                    "last_seen": null
                }
            ]
        }"#;
        let catalog: TelemetryCatalog = serde_json::from_str(json).unwrap();
        assert!(catalog.available);
        assert_eq!(catalog.series.len(), 2);
        assert_eq!(catalog.series[0].metric, "soil_moisture");
        assert_eq!(catalog.series[1].pred_dev, None);
        let back = serde_json::to_value(&catalog).unwrap();
        assert_eq!(
            back,
            serde_json::from_str::<serde_json::Value>(json).unwrap()
        );

        let json = r#"{
            "available": true,
            "metric": "soil_moisture",
            "device_id": "fuzzy-zebra",
            "points": [
                { "ts": 1787151900.0, "value": 100.0 },
                { "ts": 1787152200.0, "value": 99.5 }
            ]
        }"#;
        let series: TelemetrySeriesResponse = serde_json::from_str(json).unwrap();
        assert!(series.available);
        assert_eq!(series.points.len(), 2);
        assert_eq!(series.points[1].value, 99.5);
        let back = serde_json::to_value(&series).unwrap();
        assert_eq!(
            back,
            serde_json::from_str::<serde_json::Value>(json).unwrap()
        );
    }

    /// Org without O2 / timeout: the page must be able to draw itself with
    /// this form alone (degraded panel).
    #[test]
    fn telemetry_minimal_degrade() {
        let catalog: TelemetryCatalog =
            serde_json::from_str(r#"{ "available": false, "series": [] }"#).unwrap();
        assert!(!catalog.available);
        assert!(catalog.series.is_empty());

        let series: TelemetrySeriesResponse = serde_json::from_str(
            r#"{ "available": false, "metric": "soil_moisture", "device_id": "x", "points": [] }"#,
        )
        .unwrap();
        assert!(!series.available);
        assert!(series.points.is_empty());
    }
}
