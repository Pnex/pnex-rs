//! Encodage Prometheus remote-write des points d'ingestion — l'ingestion
//! télémétrie va dans les **metrics** OpenObserve (`/prometheus/api/v1/write`),
//! pas dans les logs : les points deviennent des séries
//! `metric_name{device_id, pred_dev, source_type, ts_source}`.
//!
//! Les structs prompb et `sanitize_metric_name` vivent dans pnex-core
//! (features `prompb`/`naming`) — source unique partagée avec le nœud
//! `metric` du runtime de flows (Phase 6), qui projette ses séries `etl_*`
//! sur les mêmes structs.

use prost::Message;

use pnex_core::{sanitize_metric_name, Label, Sample, TimeSeries, WriteRequest};

use crate::services::telemetry::TelemetryPoint;

/// Un point → une série (labels dimensions, sample value + ts serveur ms).
fn series_of(point: &TelemetryPoint) -> Option<TimeSeries> {
    let value: f64 = point.value.trim().parse().ok()?;
    let labels = vec![
        Label {
            name: "__name__".into(),
            value: sanitize_metric_name(&point.metric_name),
        },
        Label {
            name: "device_id".into(),
            value: point.device_id.clone(),
        },
        Label {
            name: "pred_dev".into(),
            value: point.pred_dev.clone(),
        },
        Label {
            name: "source_type".into(),
            value: point.source_type.to_string(),
        },
        Label {
            name: "ts_source".into(),
            value: point.ts_source.to_string(),
        },
    ];
    Some(TimeSeries {
        labels,
        samples: vec![Sample {
            value,
            timestamp: point.timestamp.timestamp_millis(),
        }],
    })
}

/// Encode un lot de points (None si tous non numériques).
pub fn encode(points: &[TelemetryPoint]) -> Option<Vec<u8>> {
    let timeseries: Vec<TimeSeries> = points.iter().filter_map(series_of).collect();
    if timeseries.is_empty() {
        return None;
    }
    Some(WriteRequest { timeseries }.encode_to_vec())
}

/// A sample with free labels (D171). Label keys are code constants, so
/// the key set is a whitelist by construction; the caller bounds the
/// cardinality of the values.
pub struct LabelledPoint {
    pub name: &'static str,
    pub labels: Vec<(&'static str, String)>,
    pub value: f64,
    pub timestamp_ms: i64,
}

/// Encodes free-label samples (non-finite values dropped; `None` if none).
pub fn encode_labelled(points: &[LabelledPoint]) -> Option<Vec<u8>> {
    let timeseries: Vec<TimeSeries> = points
        .iter()
        .filter(|p| p.value.is_finite())
        .map(|p| TimeSeries {
            labels: std::iter::once(Label {
                name: "__name__".into(),
                value: sanitize_metric_name(p.name),
            })
            .chain(p.labels.iter().map(|(k, v)| Label {
                name: (*k).into(),
                value: v.clone(),
            }))
            .collect(),
            samples: vec![Sample {
                value: p.value,
                timestamp: p.timestamp_ms,
            }],
        })
        .collect();
    if timeseries.is_empty() {
        return None;
    }
    Some(WriteRequest { timeseries }.encode_to_vec())
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn point(metric: &str, value: &str) -> TelemetryPoint {
        TelemetryPoint {
            org_id: 1,
            device_registry_id: 1,
            device_id: "capteur-1".into(),
            pred_dev: "temp_sensor".into(),
            metric_name: metric.into(),
            value: value.into(),
            timestamp: chrono::Utc.timestamp_opt(1_786_890_000, 0).unwrap(),
            ts_source: "server",
            source_type: "sensor",
            record: true,
        }
    }

    /// Roundtrip : les points numériques sortent en séries nommées et
    /// labellisées, les non numériques sont écartés.
    #[test]
    fn roundtrip_series_et_labels() {
        let mut req = WriteRequest::decode(
            encode(&[point("soil_moisture", "42.5")])
                .unwrap()
                .as_slice(),
        )
        .expect("decode");
        assert_eq!(req.timeseries.len(), 1);
        let ts = req.timeseries.remove(0);
        let name = ts
            .labels
            .iter()
            .find(|l| l.name == "__name__")
            .unwrap()
            .value
            .clone();
        assert_eq!(name, "soil_moisture");
        assert!(ts
            .labels
            .iter()
            .any(|l| l.name == "device_id" && l.value == "capteur-1"));
        assert_eq!(ts.samples[0].value, 42.5);

        assert!(encode(&[point("x", "n/a")]).is_none());
    }

    #[test]
    fn labelled_points_keep_their_labels() {
        let pb = encode_labelled(&[
            LabelledPoint {
                name: "media_capture_up",
                labels: vec![("stream", "inter".into())],
                value: 1.0,
                timestamp_ms: 5,
            },
            LabelledPoint {
                name: "media_capture_gap_seconds",
                labels: vec![],
                value: f64::NAN,
                timestamp_ms: 5,
            },
        ])
        .unwrap();
        let req = WriteRequest::decode(pb.as_slice()).unwrap();
        assert_eq!(req.timeseries.len(), 1, "NaN dropped");
        let labels: Vec<(String, String)> = req.timeseries[0]
            .labels
            .iter()
            .map(|l| (l.name.clone(), l.value.clone()))
            .collect();
        assert_eq!(
            labels,
            [
                ("__name__".into(), "media_capture_up".into()),
                ("stream".into(), "inter".into())
            ]
        );
        assert!(encode_labelled(&[]).is_none());
    }

    /// Noms de métriques invalides assainis (tirets, espace, préfixe
    /// chiffre) — les dimensions restent telles quelles.
    #[test]
    fn noms_de_metriques_assainis() {
        assert_eq!(sanitize_metric_name("soil-moisture"), "soil_moisture");
        assert_eq!(sanitize_metric_name("temp extérieure"), "temp_ext_rieure");
        assert_eq!(sanitize_metric_name("2ph"), "_ph");
        assert_eq!(sanitize_metric_name(""), "m");
    }
}
