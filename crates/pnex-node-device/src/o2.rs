//! OpenObserve mini-client of the flow runtime — the only two calls the
//! `device`/`metric` nodes need: an instant PromQL read
//! (`last_over_time`) and a remote-write.
//!
//! Why a mini-client rather than the backend client: `pnex-backend`
//! carries Loco/SeaORM/settings — an impossible dependency for the runtime.
//! Root Basic auth (OPENOBSERVE_URL/ROOT_EMAIL/ROOT_PASSWORD in the
//! runtime env allowlist): the backend's e2e finding (`client.rs ::
//! get_with_auth_fallback`) is that the org passcode does not cover
//! O2 v0.92.1 reads; root covers reads and writes.
//!
//! Every method is bounded (timeout) and returns `Result<_, String>` —
//! never a panic.

use std::time::Duration;

use base64::Engine;
use edgelink_core::EdgelinkError;
use prost::Message;

/// Client HTTP OpenObserve — auth Basic racine, timeout 10 s.
pub struct O2Client {
    base: String,
    basic: String,
    http: reqwest::Client,
}

/// One sample read: value + epoch milliseconds.
pub type LastSample = (f64, i64);

impl O2Client {
    /// Built from the runtime env. Typed error if a variable
    /// is missing (node rejected at build → flow not deployed, visible cause).
    pub fn from_env() -> Result<Self, EdgelinkError> {
        let missing = [
            "OPENOBSERVE_URL",
            "OPENOBSERVE_ROOT_EMAIL",
            "OPENOBSERVE_ROOT_PASSWORD",
        ]
        .into_iter()
        .filter(|k| std::env::var(k).is_err())
        .collect::<Vec<_>>()
        .join(", ");
        if !missing.is_empty() {
            return Err(EdgelinkError::InvalidOperation(format!(
                "pnex-device/metric : missing environment variables: {missing} \
                 (supervisor allowlist)"
            )));
        }
        let base = std::env::var("OPENOBSERVE_URL").unwrap_or_default();
        let email = std::env::var("OPENOBSERVE_ROOT_EMAIL").unwrap_or_default();
        let password = std::env::var("OPENOBSERVE_ROOT_PASSWORD").unwrap_or_default();
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(10))
            .build()
            .map_err(|e| EdgelinkError::InvalidOperation(format!("O2 client: {e}")))?;
        let basic = base64::engine::general_purpose::STANDARD.encode(format!("{email}:{password}"));
        Ok(Self { base, basic, http })
    }

    /// Last value of a series within the window — instant PromQL
    /// `last_over_time(<metric>{device_id="…"}[<w>s])`. `None` = no
    /// data in the window (key omitted from the payload, never an invented zero).
    pub async fn query_last(
        &self,
        org: &str,
        metric: &str,
        device_id: &str,
        window_secs: f64,
    ) -> Result<Option<LastSample>, String> {
        let promql = format!(
            r#"last_over_time({metric}{{device_id="{device_id}"}}[{window}s])"#,
            window = window_secs as i64
        );
        let resp = self
            .http
            .get(format!("{}/api/{org}/prometheus/api/v1/query", self.base))
            .header("Authorization", format!("Basic {}", self.basic))
            .query(&[("query", promql.as_str())])
            .send()
            .await
            .map_err(|e| format!("query O2 unreachable: {e}"))?;
        let status = resp.status();
        let body = resp
            .text()
            .await
            .map_err(|e| format!("query O2: reading the body: {e}"))?;
        if !status.is_success() {
            return Err(format!("query O2 {status} : {body}"));
        }
        parse_instant_response(&body)
    }

    /// Remote-write: encodes prompb, compresses with snappy, POSTs. `Ok(())` after
    /// 2xx.
    pub async fn write(
        &self,
        org: &str,
        timeseries: Vec<pnex_core::TimeSeries>,
    ) -> Result<(), String> {
        let req = pnex_core::WriteRequest { timeseries };
        let pb = req.encode_to_vec();
        let mut encoder = snap::raw::Encoder::new();
        let compressed = encoder
            .compress_vec(&pb)
            .map_err(|e| format!("snappy : {e}"))?;
        let resp = self
            .http
            .post(format!("{}/api/{org}/prometheus/api/v1/write", self.base))
            .header("Authorization", format!("Basic {}", self.basic))
            .header("Content-Encoding", "snappy")
            .header("Content-Type", "application/x-protobuf")
            .body(compressed)
            .send()
            .await
            .map_err(|e| format!("write O2 unreachable: {e}"))?;
        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(format!("write O2 {status} : {body}"));
        }
        Ok(())
    }
}

/// Builds the prompb series of an ETL result — labels identical to
/// sensor ingestion (device_id dimension = the flow's virtual device).
pub fn etl_series(
    metric: String,
    virtual_device: String,
    value: f64,
    ts_ms: i64,
) -> pnex_core::TimeSeries {
    etl_series_labelled(metric, virtual_device, value, ts_ms, &Default::default())
}

/// [`etl_series`] plus free labels (D171), already validated by the caller.
pub fn etl_series_labelled(
    metric: String,
    virtual_device: String,
    value: f64,
    ts_ms: i64,
    extra: &std::collections::BTreeMap<String, String>,
) -> pnex_core::TimeSeries {
    use pnex_core::{Label, Sample};
    let mut series = pnex_core::TimeSeries {
        labels: vec![
            Label {
                name: "__name__".into(),
                value: metric,
            },
            Label {
                name: "device_id".into(),
                value: virtual_device,
            },
            Label {
                name: "pred_dev".into(),
                value: "virtual_device".into(),
            },
            Label {
                name: "source_type".into(),
                value: "etl".into(),
            },
            Label {
                name: "ts_source".into(),
                value: "server".into(),
            },
        ],
        samples: vec![Sample {
            value,
            timestamp: ts_ms,
        }],
    };
    series
        .labels
        .extend(extra.iter().map(|(name, value)| Label {
            name: name.clone(),
            value: value.clone(),
        }));
    series
}

/// Parses the instant query API response: samples live under
/// `data.result[].value = [ts, "val"]` (resultType vector).
pub fn parse_instant_response(body: &str) -> Result<Option<LastSample>, String> {
    let v: serde_json::Value =
        serde_json::from_str(body).map_err(|e| format!("O2 response not JSON: {e}"))?;
    if v["status"] != "success" {
        return Err(format!(
            "query O2 : {}",
            v["error"].as_str().unwrap_or("unknown error")
        ));
    }
    let samples = v["data"]["result"]
        .as_array()
        .ok_or("O2 response: data.result missing")?;
    let Some(first) = samples.first() else {
        return Ok(None);
    };
    let raw = first["value"][1]
        .as_str()
        .ok_or("O2 response: sample without text value")?;
    let value: f64 = raw
        .parse()
        .map_err(|_| format!("O2 response: non-numeric value \"{raw}\""))?;
    let ts = first["value"][0].as_f64().unwrap_or(0.0);
    Ok(Some((value, (ts * 1000.0) as i64)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_reponse_instantanee() {
        let body = r#"{"status":"success","data":{"resultType":"vector","result":[
            {"metric":{"__name__":"d1","device_id":"cap-1"},"value":[1786890000.123,"42.5"]}
        ]}}"#;
        let (value, ts) = parse_instant_response(body).unwrap().unwrap();
        assert_eq!(value, 42.5);
        assert_eq!(ts, 1_786_890_000_123);
    }

    #[test]
    fn parse_serie_absente() {
        let body = r#"{"status":"success","data":{"resultType":"vector","result":[]}}"#;
        assert!(parse_instant_response(body).unwrap().is_none());
    }

    #[test]
    fn parse_erreur_promql() {
        let body = r#"{"status":"error","error":"parse error"}"#;
        assert!(parse_instant_response(body).is_err());
    }
}
