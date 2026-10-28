//! Prometheus remote-write encoding (WriteRequest protobuf) — prompb
//! messages shared backend ↔ flow runtime (Phase 6 ETL).
//!
//! Behind the `prompb` feature (optional `prost` dep): the wasm front never
//! compiles this module (pnex-frontend depends on pnex-core without the
//! feature), the backend and `pnex-node-device` enable it. The encoding of the
//! ingestion `TelemetryPoint`s stays in the backend (`services/
//! openobserve/promwrite.rs`); the runtime's `metric` node builds its
//! series directly on these shared structs.
//!
//! Telemetry ingestion goes into the OpenObserve **metrics**
//! (`/prometheus/api/v1/write`): the points become
//! `metric_name{device_id, pred_dev, source_type, ts_source}`.

use prost::Message;

// prompb messages (prometheus/prompb/types.proto) — hand-written prost
// encoding, only the fields in use are modeled.
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

    /// Encode/decode roundtrip of a labeled series (the runtime's `metric`
    /// node writes exactly this form).
    #[test]
    fn roundtrip_labelled_series() {
        let req = WriteRequest {
            timeseries: vec![TimeSeries {
                labels: vec![
                    Label {
                        name: "__name__".into(),
                        value: "etl_moyenne".into(),
                    },
                    Label {
                        name: "device_id".into(),
                        value: "flow_12".into(),
                    },
                ],
                samples: vec![Sample {
                    value: 21.5,
                    timestamp: 1_786_890_000_000,
                }],
            }],
        };
        let bytes = req.encode_to_vec();
        let back = WriteRequest::decode(bytes.as_slice()).expect("decode");
        assert_eq!(back, req);
    }
}
