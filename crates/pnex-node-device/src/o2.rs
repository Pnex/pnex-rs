//! Mini-client OpenObserve du runtime de flows — les deux seuls appels dont
//! les nœuds `device`/`metric` ont besoin : une lecture PromQL instantanée
//! (`last_over_time`) et un remote-write.
//!
//! Pourquoi un mini-client plutôt que le client du backend : `pnex-backend`
//! porte Loco/SeaORM/settings — une dépendance impossible pour le runtime.
//! Auth par Basic racine (OPENOBSERVE_URL/ROOT_EMAIL/ROOT_PASSWORD dans la
//! allowlist env du runtime) : le constat e2e du backend (`client.rs ::
//! get_with_auth_fallback`) est que le passcode d'org ne couvre pas les
//! lectures O2 v0.92.1 ; la racine couvre lecture et écriture.
//!
//! Toute méthode est bornée (timeout) et renvoie `Result<_, String>` —
//! jamais de panic.

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

/// Un échantillon lu : valeur + millisecondes epoch.
pub type LastSample = (f64, i64);

impl O2Client {
    /// Construit depuis l'env du runtime. Erreur typée si une variable
    /// manque (nœud rejeté au build → flow non déployé, cause visible).
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
                "pnex-device/metric : variables d'environnement absentes : {missing} \
                 (allowlist du superviseur)"
            )));
        }
        let base = std::env::var("OPENOBSERVE_URL").unwrap_or_default();
        let email = std::env::var("OPENOBSERVE_ROOT_EMAIL").unwrap_or_default();
        let password = std::env::var("OPENOBSERVE_ROOT_PASSWORD").unwrap_or_default();
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(10))
            .build()
            .map_err(|e| EdgelinkError::InvalidOperation(format!("client O2 : {e}")))?;
        let basic = base64::engine::general_purpose::STANDARD
            .encode(format!("{email}:{password}"));
        Ok(Self { base, basic, http })
    }

    /// Dernière valeur d'une série dans la fenêtre — PromQL instantané
    /// `last_over_time(<metric>{device_id="…"}[<w>s])`. `None` = aucune
    /// donnée dans la fenêtre (clé omise du payload, jamais de zéro inventé).
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
            .map_err(|e| format!("query O2 injoignable : {e}"))?;
        let status = resp.status();
        let body = resp.text().await.map_err(|e| format!("query O2 : lecture du corps : {e}"))?;
        if !status.is_success() {
            return Err(format!("query O2 {status} : {body}"));
        }
        parse_instant_response(&body)
    }

    /// Remote-write : encode prompb, compresse snappy, POSTe. `Ok(())` après
    /// 2xx.
    pub async fn write(
        &self,
        org: &str,
        timeseries: Vec<pnex_core::TimeSeries>,
    ) -> Result<(), String> {
        let req = pnex_core::WriteRequest { timeseries };
        let pb = req.encode_to_vec();
        let mut encoder = snap::raw::Encoder::new();
        let compressed = encoder.compress_vec(&pb).map_err(|e| format!("snappy : {e}"))?;
        let resp = self
            .http
            .post(format!("{}/api/{org}/prometheus/api/v1/write", self.base))
            .header("Authorization", format!("Basic {}", self.basic))
            .header("Content-Encoding", "snappy")
            .header("Content-Type", "application/x-protobuf")
            .body(compressed)
            .send()
            .await
            .map_err(|e| format!("write O2 injoignable : {e}"))?;
        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(format!("write O2 {status} : {body}"));
        }
        Ok(())
    }
}

/// Construit la série prompb d'un résultat ETL — labels identiques à
/// l'ingestion capteur (dimension device_id = device virtuel du flow).
pub fn etl_series(
    metric: String,
    virtual_device: String,
    value: f64,
    ts_ms: i64,
) -> pnex_core::TimeSeries {
    use pnex_core::{Label, Sample};
    pnex_core::TimeSeries {
        labels: vec![
            Label { name: "__name__".into(), value: metric },
            Label { name: "device_id".into(), value: virtual_device },
            Label { name: "pred_dev".into(), value: "virtual_device".into() },
            Label { name: "source_type".into(), value: "etl".into() },
            Label { name: "ts_source".into(), value: "server".into() },
        ],
        samples: vec![Sample { value, timestamp: ts_ms }],
    }
}

/// Parse la réponse de l'API query instantanée : les samples vivent sous
/// `data.result[].value = [ts, "val"]` (resultType vector).
pub fn parse_instant_response(body: &str) -> Result<Option<LastSample>, String> {
    let v: serde_json::Value =
        serde_json::from_str(body).map_err(|e| format!("réponse O2 non JSON : {e}"))?;
    if v["status"] != "success" {
        return Err(format!("query O2 : {}", v["error"].as_str().unwrap_or("erreur inconnue")));
    }
    let samples = v["data"]["result"].as_array().ok_or("réponse O2 : data.result absent")?;
    let Some(first) = samples.first() else {
        return Ok(None);
    };
    let raw = first["value"][1]
        .as_str()
        .ok_or("réponse O2 : sample sans valeur texte")?;
    let value: f64 = raw
        .parse()
        .map_err(|_| format!("réponse O2 : valeur non numérique « {raw} »"))?;
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
