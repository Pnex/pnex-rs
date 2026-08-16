//! Encodage Prometheus remote-write (WriteRequest protobuf) — l'ingestion
//! télémétrie va dans les **metrics** OpenObserve (`/prometheus/api/v1/write`),
//! pas dans les logs : les points deviennent des séries
//! `metric_name{device_id, pred_dev, source_type, ts_source}`.

use prost::Message;

use crate::services::telemetry::TelemetryPoint;

// Messages prompb (prometheus/prompb/types.proto) — encodage prost à la
// main, seuls les champs utilisés sont modélisés.
#[derive(Clone, PartialEq, prost::Message)]
pub struct WriteRequest {
    #[prost(message, repeated, tag = "1")]
    pub timeseries: Vec<TimeSeries>,
}

#[derive(Clone, PartialEq, prost::Message)]
pub struct TimeSeries {
    #[prost(message, repeated, tag = "1")]
    pub labels: Vec<Label>,
    #[prost(message, repeated, tag = "2")]
    pub samples: Vec<Sample>,
}

#[derive(Clone, PartialEq, prost::Message)]
pub struct Label {
    #[prost(string, tag = "1")]
    pub name: String,
    #[prost(string, tag = "2")]
    pub value: String,
}

#[derive(Clone, PartialEq, prost::Message)]
pub struct Sample {
    #[prost(double, tag = "1")]
    pub value: f64,
    /// Millisecondes epoch.
    #[prost(int64, tag = "2")]
    pub timestamp: i64,
}

/// Nom de métrique Prometheus valide : `[a-zA-Z_:][a-zA-Z0-9_:]*` — les
/// caractères interdits deviennent `_`, un préfixe `_` interdit est
/// évité (interdiction de ressembler aux séries internes).
pub fn sanitize_metric_name(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    for (i, c) in name.chars().enumerate() {
        let valid = c.is_ascii_alphanumeric() || c == '_' || c == ':';
        if valid && !(i == 0 && c.is_ascii_digit()) {
            out.push(c);
        } else {
            out.push('_');
        }
    }
    if out.is_empty() {
        out.push('m');
    }
    out
}

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

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn point(metric: &str, value: &str) -> TelemetryPoint {
        TelemetryPoint {
            org_id: 1,
            device_registry_id: 1,
            device_id: "capteur-1".into(),
            pred_dev: "soil_sensor".into(),
            metric_name: metric.into(),
            value: value.into(),
            timestamp: chrono::Utc.timestamp_opt(1_786_890_000, 0).unwrap(),
            ts_source: "server",
            source_type: "sensor",
        }
    }

    /// Roundtrip : les points numériques sortent en séries nommées et
    /// labellisées, les non numériques sont écartés.
    #[test]
    fn roundtrip_series_et_labels() {
        let mut req = WriteRequest::decode(encode(&[point("soil_moisture", "42.5")])
            .unwrap()
            .as_slice())
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
        assert!(ts.labels.iter().any(|l| l.name == "device_id" && l.value == "capteur-1"));
        assert_eq!(ts.samples[0].value, 42.5);

        assert!(encode(&[point("x", "n/a")]).is_none());
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
