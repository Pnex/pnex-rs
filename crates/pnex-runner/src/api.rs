//! HTTP client of the runner routes (`/api/v1/runner/...`): always outbound, so a
//! runner behind NAT needs no open port.

use std::time::Duration;

use pnex_core::runners::{
    RunnerClaimedJob, RunnerJobFinish, RunnerJobOutcome, RunnerRegister, RunnerRegistered,
};
use reqwest::StatusCode;

use crate::{Api, ApiError};

const TIMEOUT: Duration = Duration::from_secs(20);

pub struct HttpApi {
    http: reqwest::Client,
    base: String,
    token: String,
}

fn client() -> Result<reqwest::Client, String> {
    reqwest::Client::builder()
        .timeout(TIMEOUT)
        .build()
        .map_err(|e| e.to_string())
}

/// Consumes the join token. The error never repeats a token.
pub async fn register(
    base: &str,
    join_token: &str,
    capabilities: Vec<String>,
) -> Result<RunnerRegistered, String> {
    let res = client()?
        .post(format!("{base}/api/v1/runner/register"))
        .json(&RunnerRegister {
            join_token: join_token.to_string(),
            capabilities,
        })
        .send()
        .await
        .map_err(|e| e.without_url().to_string())?;
    if !res.status().is_success() {
        return Err(format!("registration refused: {}", res.status()));
    }
    res.json().await.map_err(|e| e.without_url().to_string())
}

impl HttpApi {
    pub fn new(base: &str, token: String) -> Result<Self, String> {
        Ok(Self {
            http: client()?,
            base: base.to_string(),
            token,
        })
    }

    async fn post(
        &self,
        path: &str,
        body: Option<&RunnerJobFinish>,
    ) -> Result<reqwest::Response, ApiError> {
        let mut req = self
            .http
            .post(format!("{}/api/v1/runner/{path}", self.base))
            .bearer_auth(&self.token);
        if let Some(body) = body {
            req = req.json(body);
        }
        let res = req
            .send()
            .await
            .map_err(|e| ApiError::Other(e.without_url().to_string()))?;
        match res.status() {
            s if s.is_success() => Ok(res),
            StatusCode::CONFLICT => Err(ApiError::NotHeld),
            s => Err(ApiError::Other(format!("{path}: {s}"))),
        }
    }
}

impl Api for HttpApi {
    async fn claim(&self) -> Result<Option<RunnerClaimedJob>, ApiError> {
        let res = self.post("jobs/claim", None).await?;
        if res.status() == StatusCode::NO_CONTENT {
            return Ok(None);
        }
        res.json()
            .await
            .map(Some)
            .map_err(|e| ApiError::Other(e.without_url().to_string()))
    }

    async fn upload_sbom(&self, job: &str, document: String) -> Result<(), ApiError> {
        let res = self
            .http
            .post(format!("{}/api/v1/runner/jobs/{job}/sbom", self.base))
            .bearer_auth(&self.token)
            .header("content-type", "application/json")
            .body(document)
            .send()
            .await
            .map_err(|e| ApiError::Other(e.without_url().to_string()))?;
        match res.status() {
            s if s.is_success() => Ok(()),
            StatusCode::CONFLICT => Err(ApiError::NotHeld),
            s => Err(ApiError::Other(format!("sbom: {s}"))),
        }
    }

    async fn upload_logs(
        &self,
        job: &str,
        logs: &pnex_core::runners::RunnerJobLogs,
    ) -> Result<(), ApiError> {
        let res = self
            .http
            .post(format!("{}/api/v1/runner/jobs/{job}/logs", self.base))
            .bearer_auth(&self.token)
            .json(logs)
            .send()
            .await
            .map_err(|e| ApiError::Other(e.without_url().to_string()))?;
        match res.status() {
            s if s.is_success() => Ok(()),
            StatusCode::CONFLICT => Err(ApiError::NotHeld),
            s => Err(ApiError::Other(format!("logs: {s}"))),
        }
    }

    async fn heartbeat(&self, job: &str) -> Result<(), ApiError> {
        self.post(&format!("jobs/{job}/heartbeat"), None)
            .await
            .map(|_| ())
    }

    async fn finish(
        &self,
        job: &str,
        outcome: RunnerJobOutcome,
        exit_code: Option<i32>,
        image_digest: Option<String>,
        scan_report: Option<pnex_core::environments::ScanReport>,
    ) -> Result<(), ApiError> {
        self.post(
            &format!("jobs/{job}/finish"),
            Some(&RunnerJobFinish {
                outcome,
                exit_code,
                image_digest,
                scan_report,
            }),
        )
        .await
        .map(|_| ())
    }
}
