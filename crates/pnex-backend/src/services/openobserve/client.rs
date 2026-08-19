//! Client HTTP OpenObserve (admin pour le provisioning, Basic pour
//! l'ingestion). Erreurs remontées en texte — relayées dans
//! `openobserve_orgs.last_error`.

use std::collections::HashMap;

use base64::engine::general_purpose::STANDARD;
use base64::Engine as _;
use serde::Deserialize;

use super::OpenobserveSettings;

#[derive(Debug, Deserialize)]
struct OrgsResponse {
    data: Vec<OrgRow>,
}

#[derive(Debug, Deserialize)]
pub struct OrgRow {
    pub identifier: String,
    pub name: String,
}

#[derive(Debug, Deserialize)]
struct PasscodeResponse {
    data: PasscodeData,
}

/// Réponse d'une requête instantanée Prometheus (`/api/v1/query`) —
/// forme vector attendue : un échantillon par série active.
#[derive(Debug, Clone, Deserialize)]
pub struct PromQueryResponse {
    pub status: String,
    pub data: PromQueryData,
}

#[derive(Debug, Clone, Deserialize)]
pub struct PromQueryData {
    #[serde(rename = "resultType")]
    pub result_type: String,
    pub result: Vec<PromQuerySample>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct PromQuerySample {
    /// Labels de la série — `__name__` + labels portés à l'ingest
    /// (`device_id`, `pred_dev`, `source_type`, `ts_source`).
    pub metric: HashMap<String, String>,
    /// (timestamp en secondes epoch, valeur en texte — re-parse
    /// défensif côté consommateur).
    pub value: (f64, String),
}

/// Réponse d'une requête range Prometheus
/// (`/api/v1/query_range`) — une série porte sa liste de points.
#[derive(Debug, Clone, Deserialize)]
pub struct PromRangeResponse {
    pub status: String,
    pub data: PromRangeData,
}

#[derive(Debug, Clone, Deserialize)]
pub struct PromRangeData {
    #[serde(rename = "resultType")]
    pub result_type: String,
    pub result: Vec<PromRangeSample>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct PromRangeSample {
    /// Labels de la série — `__name__` + labels portés à l'ingest.
    pub metric: HashMap<String, String>,
    /// Points `(timestamp secondes epoch, valeur texte)` — O2 ne remplit
    /// pas les trous entre deux pas : seuls les points réels sont rendus.
    pub values: Vec<(f64, String)>,
}

/// Réponse de `GET /api/{org}/streams?type=metrics` — seuls les noms nous
/// intéressent (les stats/schema sont ignorés par serde).
#[derive(Debug, Clone, Deserialize)]
struct StreamsResponse {
    list: Vec<StreamRow>,
}

#[derive(Debug, Clone, Deserialize)]
struct StreamRow {
    name: String,
}

#[derive(Debug, Deserialize)]
struct PasscodeData {
    passcode: String,
}

/// Client OpenObserve — un pour le boot (root), réutilisable pour
/// l'ingestion avec le Basic `email:passcode` stocké en base.
#[derive(Clone)]
pub struct Client {
    base: String,
    root_basic: String,
    http: reqwest::Client,
}

fn basic(email: &str, password: &str) -> String {
    format!("Basic {}", STANDARD.encode(format!("{email}:{password}")))
}

impl Client {
    pub fn new(settings: &OpenobserveSettings) -> Self {
        Self {
            base: settings.base_url.trim_end_matches('/').to_string(),
            root_basic: basic(&settings.root_email, &settings.root_password),
            http: reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(10))
                .build()
                .expect("client http openobserve"),
        }
    }

    async fn json_request(
        &self,
        method: reqwest::Method,
        path: &str,
        auth: &str,
        body: Option<serde_json::Value>,
    ) -> Result<(reqwest::StatusCode, String), String> {
        let mut req = self
            .http
            .request(method, format!("{}{path}", self.base))
            .header("Authorization", auth);
        if let Some(json) = body {
            req = req.json(&json);
        }
        let resp = req
            .send()
            .await
            .map_err(|e| format!("openobserve injoignable : {e}"))?;
        let status = resp.status();
        let text = resp.text().await.unwrap_or_default();
        Ok((status, text))
    }

    /// Orgs existantes (identifier + name).
    pub async fn organizations(&self) -> Result<Vec<OrgRow>, String> {
        let (status, text) = self
            .json_request(
                reqwest::Method::GET,
                "/api/organizations",
                &self.root_basic,
                None,
            )
            .await?;
        if !status.is_success() {
            return Err(format!("list orgs {status} : {text}"));
        }
        serde_json::from_str::<OrgsResponse>(&text)
            .map(|r| r.data)
            .map_err(|e| format!("list orgs illisible : {e}"))
    }

    /// Identifier d'une org par nom — O2 ne dédoublonne pas les noms, on
    /// cherche TOUJOURS avant de créer.
    pub async fn find_org_by_name(&self, name: &str) -> Result<Option<String>, String> {
        Ok(self
            .organizations()
            .await?
            .into_iter()
            .find(|o| o.name == name)
            .map(|o| o.identifier))
    }

    /// Crée l'org → identifier.
    pub async fn create_org(&self, name: &str) -> Result<String, String> {
        let (status, text) = self
            .json_request(
                reqwest::Method::POST,
                "/api/organizations",
                &self.root_basic,
                Some(serde_json::json!({ "name": name })),
            )
            .await?;
        if !status.is_success() {
            return Err(format!("create org {status} : {text}"));
        }
        serde_json::from_str::<OrgRow>(&text)
            .map(|o| o.identifier)
            .map_err(|e| format!("create org illisible : {e}"))
    }

    /// Crée le user d'ingestion (role admin, seul natif). Ok(false) =
    /// existait déjà (« User already exists »).
    pub async fn create_user(
        &self,
        org_identifier: &str,
        email: &str,
        password: &str,
    ) -> Result<bool, String> {
        let (status, text) = self
            .json_request(
                reqwest::Method::POST,
                &format!("/api/{org_identifier}/users"),
                &self.root_basic,
                Some(serde_json::json!({
                    "email": email, "password": password, "role": "admin"
                })),
            )
            .await?;
        if text.contains("User already exists") {
            return Ok(false);
        }
        if !status.is_success() {
            return Err(format!("create user {status} : {text}"));
        }
        Ok(true)
    }

    /// Root reprend un user sans son ancien mot de passe (ligne PG perdue).
    pub async fn reset_user_password(
        &self,
        org_identifier: &str,
        email: &str,
        new_password: &str,
    ) -> Result<(), String> {
        let (status, text) = self
            .json_request(
                reqwest::Method::PUT,
                &format!("/api/{org_identifier}/users/{email}"),
                &self.root_basic,
                Some(serde_json::json!({
                    "email": email, "new_password": new_password, "change_password": true
                })),
            )
            .await?;
        if !status.is_success() {
            return Err(format!("reset user {status} : {text}"));
        }
        Ok(())
    }

    /// Passcode du user (auth Basic email:password du user lui-même).
    pub async fn passcode(
        &self,
        org_identifier: &str,
        email: &str,
        password: &str,
    ) -> Result<String, String> {
        let (status, text) = self
            .json_request(
                reqwest::Method::GET,
                &format!("/api/{org_identifier}/passcode"),
                &basic(email, password),
                None,
            )
            .await?;
        if !status.is_success() {
            return Err(format!("passcode {status} : {text}"));
        }
        serde_json::from_str::<PasscodeResponse>(&text)
            .map(|r| r.data.passcode)
            .map_err(|e| format!("passcode illisible : {e}"))
    }

    /// Ingestion d'un lot Prometheus remote-write (protobuf compressé
    /// snappy) — les points atterrissent dans les **metrics** de l'org,
    /// avec le Basic `email:passcode` stocké dans
    /// `openobserve_orgs.ingestion_token`.
    pub async fn ingest_prometheus(
        &self,
        org_identifier: &str,
        write_request_pb: &[u8],
        email_passcode: &str,
    ) -> Result<(), String> {
        let compressed = snap::raw::Encoder::new()
            .compress_vec(write_request_pb)
            .map_err(|e| format!("snappy : {e}"))?;
        let resp = self
            .http
            .post(format!(
                "{}/api/{org_identifier}/prometheus/api/v1/write",
                self.base
            ))
            .header(
                "Authorization",
                format!("Basic {}", STANDARD.encode(email_passcode)),
            )
            .header("Content-Encoding", "snappy")
            .header("Content-Type", "application/x-protobuf")
            .body(compressed)
            .send()
            .await
            .map_err(|e| format!("ingest injoignable : {e}"))?;
        let status = resp.status();
        if !status.is_success() {
            let text = resp.text().await.unwrap_or_default();
            return Err(format!("ingest prometheus {status} : {text}"));
        }
        Ok(())
    }

    /// GET authentifié avec bascule : Basic `email:passcode` d'abord, sur
    /// 401/403 retry en Basic root — **c'est le chemin réel** : constaté
    /// e2e sur O2 v0.92.1, le Basic email:passcode est refusé sur les
    /// endpoints de lecture (il ne sert que l'ingestion). Le passcode
    /// reste tenté d'abord (moindre privilège, future-proof si O2
    /// l'accepte un jour). Retourne le corps texte en cas de succès.
    async fn get_with_auth_fallback(
        &self,
        url: &str,
        query_pairs: &[(&str, &str)],
        email_passcode: &str,
        what: &str,
    ) -> Result<String, String> {
        let passcode_basic = format!("Basic {}", STANDARD.encode(email_passcode));
        let auths = [passcode_basic.as_str(), self.root_basic.as_str()];
        let mut last_denial = String::new();
        for auth in auths {
            let resp = self
                .http
                .get(url)
                .query(query_pairs)
                .header("Authorization", auth)
                .send()
                .await
                .map_err(|e| format!("{what} injoignable : {e}"))?;
            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();
            if status.as_u16() == 401 || status.as_u16() == 403 {
                last_denial = status.to_string();
                continue;
            }
            if !status.is_success() {
                return Err(format!("{what} {status} : {text}"));
            }
            return Ok(text);
        }
        Err(format!("{what} refusé (passcode puis root) : {last_denial}"))
    }

    /// Requête instantanée Prometheus
    /// (`GET /api/{org}/prometheus/api/v1/query?query=…`) — lecture des
    /// métriques de l'org (dashboard).
    ///
    /// ⚠ Constat e2e sur O2 v0.92.1 : les sélecteurs `{__name__=~"..."}`
    /// (regex) ne renvoient **rien**, même avec des noms explicites —
    /// seul le **nom nu** (`last_over_time(soil_moisture[1h])`) ou
    /// l'égalité `{__name__="x"}` fonctionnent. Les noms à interroger se
    /// découvrent via [`Client::metric_streams`].
    pub async fn prom_query(
        &self,
        org_identifier: &str,
        query: &str,
        email_passcode: &str,
    ) -> Result<PromQueryResponse, String> {
        let text = self
            .get_with_auth_fallback(
                &format!("{}/api/{org_identifier}/prometheus/api/v1/query", self.base),
                &[("query", query)],
                email_passcode,
                "query prometheus",
            )
            .await?;
        serde_json::from_str::<PromQueryResponse>(&text)
            .map_err(|e| format!("query illisible : {e}"))
    }

    /// Requête range Prometheus
    /// (`GET /api/{org}/prometheus/api/v1/query_range?query&start&end&step`)
    /// — lecture d'une série sur une fenêtre (page Visualisation).
    ///
    /// Constat e2e v0.92.1 : le nom nu ou l'égalité
    /// (`soil_moisture{device_id="x"}`) fonctionnent — `start`/`end` en
    /// secondes epoch, `step` en secondes. O2 rend les points réels sans
    /// remplir les trous.
    pub async fn prom_query_range(
        &self,
        org_identifier: &str,
        query: &str,
        start_epoch: i64,
        end_epoch: i64,
        step_secs: i64,
        email_passcode: &str,
    ) -> Result<PromRangeResponse, String> {
        let text = self
            .get_with_auth_fallback(
                &format!("{}/api/{org_identifier}/prometheus/api/v1/query_range", self.base),
                &[
                    ("query", query),
                    ("start", &start_epoch.to_string()),
                    ("end", &end_epoch.to_string()),
                    ("step", &step_secs.to_string()),
                ],
                email_passcode,
                "query range prometheus",
            )
            .await?;
        serde_json::from_str::<PromRangeResponse>(&text)
            .map_err(|e| format!("query range illisible : {e}"))
    }

    /// Streams **metrics** de l'org (noms de métriques existantes) —
    /// `GET /api/{org}/streams?type=metrics`. Sert de découverte pour la
    /// query Prometheus (cf. `prom_query` : pas de sélecteur regex sur
    /// `__name__` en v0.92.1).
    pub async fn metric_streams(
        &self,
        org_identifier: &str,
        email_passcode: &str,
    ) -> Result<Vec<String>, String> {
        let text = self
            .get_with_auth_fallback(
                &format!("{}/api/{org_identifier}/streams", self.base),
                &[("type", "metrics")],
                email_passcode,
                "streams metrics",
            )
            .await?;
        serde_json::from_str::<StreamsResponse>(&text)
            .map(|r| r.list.into_iter().map(|s| s.name).collect())
            .map_err(|e| format!("streams illisibles : {e}"))
    }

    /// `/healthz` répond (readiness — vérifié : `/health` n'existe pas en
    /// v0.92.1, il répond 401 sans auth puis 404 avec).
    pub async fn healthy(&self) -> bool {
        self.http
            .get(format!("{}/healthz", self.base))
            .send()
            .await
            .map(|r| r.status().is_success())
            .unwrap_or(false)
    }
}
