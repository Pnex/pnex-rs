//! Encodage Prometheus remote-write (WriteRequest protobuf) — messages
//! prompb partagés backend ↔ runtime de flows (Phase 6 ETL).
//!
//! Derrière la feature `prompb` (dep `prost` optionnelle) : le front wasm ne
//! compile jamais ce module (pnex-frontend dépend de pnex-core sans la
//! feature), le backend et `pnex-node-device` l'activent. L'encodage des
//! `TelemetryPoint` d'ingestion reste dans le backend (`services/
//! openobserve/promwrite.rs`) ; le nœud `metric` du runtime construit ses
//! séries directement sur ces structs partagés.
//!
//! L'ingestion télémétrie va dans les **metrics** OpenObserve
//! (`/prometheus/api/v1/write`) : les points deviennent des séries
//! `metric_name{device_id, pred_dev, source_type, ts_source}`.

use prost::Message;

// Messages prompb (prometheus/prompb/types.proto) — encodage prost à la
// main, seuls les champs utilisés sont modélisés.
#[derive(Clone, PartialEq, Message)]
pub struct WriteRequest {
    #[prost(message, repeated, tag = "1")]
    pub timeseries: Vec<TimeSeries>,
}

#[derive(Clone, PartialEq, Message)]
pub struct TimeSeries {
    #[prost(message, repeated, tag = "1")]
    pub labels: Vec<Label>,
    #[prost(message, repeated, tag = "2")]
    pub samples: Vec<Sample>,
}

#[derive(Clone, PartialEq, Message)]
pub struct Label {
    #[prost(string, tag = "1")]
    pub name: String,
    #[prost(string, tag = "2")]
    pub value: String,
}

#[derive(Clone, PartialEq, Message)]
pub struct Sample {
    #[prost(double, tag = "1")]
    pub value: f64,
    /// Millisecondes epoch.
    #[prost(int64, tag = "2")]
    pub timestamp: i64,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Roundtrip encode/decode d'une série labellisée (le nœud `metric` du
    /// runtime écrit exactement cette forme).
    #[test]
    fn roundtrip_serie_labellisee() {
        let req = WriteRequest {
            timeseries: vec![TimeSeries {
                labels: vec![
                    Label { name: "__name__".into(), value: "etl_moyenne".into() },
                    Label { name: "device_id".into(), value: "flow_12".into() },
                ],
                samples: vec![Sample { value: 21.5, timestamp: 1_786_890_000_000 }],
            }],
        };
        let bytes = req.encode_to_vec();
        let back = WriteRequest::decode(bytes.as_slice()).expect("decode");
        assert_eq!(back, req);
    }
}
