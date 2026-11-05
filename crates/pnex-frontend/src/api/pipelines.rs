//! Data pipelines API (data-pipelines.md D571–D578): tables, pipelines, chained runs
//! and connectors.

use pnex_core::connectors::{Connector, ConnectorInput, ConnectorTest};
use std::time::Duration;

use futures::StreamExt;
use pnex_core::datasets::{
    Dataset, DatasetInput, DatasetProfile, DatasetSample, DatasetTransaction, DatasetUpload,
    DatasetUploadInput,
};
use pnex_core::pipelines::{
    Build, BuildInput, BuildPlan, BuildTargetKind, DatasetLineage, Pipeline, PipelineDocument,
    PipelineRun, PipelineVersion,
};
use pnex_core::Paginated;
use reqwest::Method;
use serde_json::json;

use crate::api::client;
use crate::api::error::ApiError;
use crate::api::media::urlencode;

fn pipeline_path(id: &str) -> String {
    format!("/api/v1/pipelines/{}", urlencode(id))
}

fn table_path(id: &str) -> String {
    format!("/api/v1/datasets/{}", urlencode(id))
}

fn connector_path(id: &str) -> String {
    format!("/api/v1/connectors/{}", urlencode(id))
}

async fn delete_at(path: &str) -> Result<(), ApiError> {
    client::request_opt::<serde_json::Value>(Method::DELETE, path, None)
        .await
        .map(|_| ())
}

pub async fn list() -> Result<Vec<Pipeline>, ApiError> {
    client::request_all("/api/v1/pipelines").await
}

pub async fn get(id: &str) -> Result<Pipeline, ApiError> {
    client::request(Method::GET, &pipeline_path(id), None).await
}

pub async fn create(name: &str, document: &str) -> Result<Pipeline, ApiError> {
    let body = json!({ "name": name, "document": document });
    client::request(Method::POST, "/api/v1/pipelines", Some(body)).await
}

/// 400 = `pipeline-document-invalid`, with its violations.
pub async fn validate(document: &str) -> Result<PipelineDocument, ApiError> {
    let body = json!({ "document": document });
    client::request(Method::POST, "/api/v1/pipelines/validate", Some(body)).await
}

/// 409 when saved in between.
pub async fn update(id: &str, document: &str, expected_version: i32) -> Result<Pipeline, ApiError> {
    let body = json!({ "document": document, "expected_version": expected_version });
    client::request(Method::PUT, &pipeline_path(id), Some(body)).await
}

pub async fn delete(id: &str) -> Result<(), ApiError> {
    delete_at(&pipeline_path(id)).await
}

pub async fn versions(id: &str) -> Result<Vec<PipelineVersion>, ApiError> {
    client::request_all(&format!("{}/versions", pipeline_path(id))).await
}

pub async fn runs(id: &str, limit: i64) -> Result<Paginated<PipelineRun>, ApiError> {
    let path = format!("{}/runs?limit={limit}", pipeline_path(id));
    client::request(Method::GET, &path, None).await
}

/// `None` stops the schedule.
pub async fn schedule(
    id: &str,
    recurrence: Option<serde_json::Value>,
) -> Result<Pipeline, ApiError> {
    let path = format!("{}/schedule", pipeline_path(id));
    let body = json!({ "recurrence": recurrence });
    client::request(Method::PUT, &path, Some(body)).await
}

pub async fn plan(kind: BuildTargetKind, id: &str) -> Result<BuildPlan, ApiError> {
    let path = format!(
        "/api/v1/data-builds/plan?target_kind={}&target_id={}",
        kind.wire(),
        urlencode(id)
    );
    client::request(Method::GET, &path, None).await
}

pub async fn start_build(target_kind: BuildTargetKind, target_id: &str) -> Result<Build, ApiError> {
    let body = BuildInput {
        target_kind,
        target_id: target_id.to_string(),
    };
    client::request(
        Method::POST,
        "/api/v1/data-builds",
        Some(serde_json::to_value(body).unwrap_or_default()),
    )
    .await
}

pub async fn build(id: &str) -> Result<Build, ApiError> {
    let path = format!("/api/v1/data-builds/{}", urlencode(id));
    client::request(Method::GET, &path, None).await
}

pub async fn builds(limit: i64, offset: i64) -> Result<Paginated<Build>, ApiError> {
    let path = format!("/api/v1/data-builds?limit={limit}&offset={offset}");
    client::request(Method::GET, &path, None).await
}

pub async fn tables() -> Result<Vec<Dataset>, ApiError> {
    client::request_all("/api/v1/datasets").await
}

pub async fn table(id: &str) -> Result<Dataset, ApiError> {
    client::request(Method::GET, &table_path(id), None).await
}

pub async fn create_table(name: &str) -> Result<Dataset, ApiError> {
    let body = DatasetInput {
        name: name.to_string(),
    };
    client::request(
        Method::POST,
        "/api/v1/datasets",
        Some(serde_json::to_value(body).unwrap_or_default()),
    )
    .await
}

pub async fn delete_table(id: &str) -> Result<(), ApiError> {
    delete_at(&table_path(id)).await
}

pub async fn sample(id: &str, as_of: Option<&str>) -> Result<DatasetSample, ApiError> {
    let at = as_of
        .map(|t| format!("&as_of={}", urlencode(t)))
        .unwrap_or_default();
    let path = format!("{}/sample?limit=50{at}", table_path(id));
    client::request(Method::GET, &path, None).await
}

pub async fn versions_of_table(
    id: &str,
    limit: i64,
) -> Result<Paginated<DatasetTransaction>, ApiError> {
    let path = format!("{}/transactions?limit={limit}", table_path(id));
    client::request(Method::GET, &path, None).await
}

pub async fn lineage(id: &str) -> Result<DatasetLineage, ApiError> {
    let path = format!("{}/lineage", table_path(id));
    client::request(Method::GET, &path, None).await
}

pub async fn profile(id: &str) -> Result<DatasetProfile, ApiError> {
    let path = format!("{}/profile", table_path(id));
    client::request(Method::GET, &path, None).await
}

/// `None` stops the check.
pub async fn set_freshness(id: &str, max_age_hours: Option<i32>) -> Result<Dataset, ApiError> {
    let path = format!("{}/freshness", table_path(id));
    let body = json!({ "max_age_hours": max_age_hours });
    client::request(Method::PUT, &path, Some(body)).await
}

/// A part stays well under the limit of a proxy on one request. A retry holds it twice.
const UPLOAD_PART_BYTES: usize = 32 * 1024 * 1024;
const PART_ATTEMPTS: u32 = 4;
/// Five minutes of failed polls: longer than a rollout.
const UPLOAD_POLL_FAILURES: u32 = 300;

fn may_pass_later(e: &ApiError) -> bool {
    e.status.is_none_or(|s| s >= 500)
}

async fn upload_part(path: &str, part: Vec<u8>) -> Result<(), ApiError> {
    let mut attempt = 1;
    loop {
        let sent = client::request_upload::<DatasetUpload>(Method::PUT, path, part.clone()).await;
        match sent {
            Ok(_) => return Ok(()),
            Err(e) if attempt < PART_ATTEMPTS && may_pass_later(&e) => {
                attempt += 1;
                crate::util::sleep(Duration::from_secs(2)).await;
            }
            Err(e) => return Err(e),
        }
    }
}

/// Sends a file in parts, then waits for its conversion. `sent` gets the bytes sent so
/// far. A failed conversion is the `failed` upload returned, not an error.
pub async fn upload_csv<S, B, E>(
    id: &str,
    delimiter: &str,
    mut file: S,
    mut sent: impl FnMut(u64),
) -> Result<DatasetUpload, ApiError>
where
    S: futures::Stream<Item = Result<B, E>> + Unpin,
    B: AsRef<[u8]>,
    E: std::fmt::Display,
{
    let uploads = format!("{}/uploads", table_path(id));
    let input = DatasetUploadInput {
        kind: None,
        delimiter: Some(delimiter.to_string()),
    };
    let body = Some(serde_json::to_value(input).unwrap_or_default());
    let upload: DatasetUpload = client::request(Method::POST, &uploads, body).await?;
    let path = format!("{uploads}/{}", upload.id);
    let (mut buffer, mut parts, mut total) = (Vec::new(), 0, 0u64);
    while let Some(chunk) = file.next().await {
        let chunk = chunk.map_err(|e| ApiError::new(e.to_string()))?;
        buffer.extend_from_slice(chunk.as_ref());
        while buffer.len() >= UPLOAD_PART_BYTES {
            let rest = buffer.split_off(UPLOAD_PART_BYTES);
            let part = std::mem::replace(&mut buffer, rest);
            parts += 1;
            total += part.len() as u64;
            upload_part(&format!("{path}/parts/{parts}"), part).await?;
            sent(total);
        }
    }
    // An empty file too is a part: the server says why it refuses it.
    if !buffer.is_empty() || parts == 0 {
        parts += 1;
        total += buffer.len() as u64;
        upload_part(&format!("{path}/parts/{parts}"), buffer).await?;
        sent(total);
    }
    let body = json!({ "parts": parts });
    let mut upload: DatasetUpload =
        client::request(Method::POST, &format!("{path}/complete"), Some(body)).await?;
    let mut failures = 0;
    while !upload.state.ended() {
        crate::util::sleep(Duration::from_secs(1)).await;
        match client::request(Method::GET, &path, None).await {
            Ok(now) => {
                upload = now;
                failures = 0;
            }
            Err(e) if may_pass_later(&e) && failures < UPLOAD_POLL_FAILURES => failures += 1,
            Err(e) => return Err(e),
        }
    }
    Ok(upload)
}

pub async fn connectors() -> Result<Vec<Connector>, ApiError> {
    client::request_all("/api/v1/connectors").await
}

pub async fn save_connector(
    id: Option<&str>,
    input: &ConnectorInput,
) -> Result<Connector, ApiError> {
    let body = Some(serde_json::to_value(input).unwrap_or_default());
    match id {
        Some(id) => client::request(Method::PUT, &connector_path(id), body).await,
        None => client::request(Method::POST, "/api/v1/connectors", body).await,
    }
}

pub async fn delete_connector(id: &str) -> Result<(), ApiError> {
    delete_at(&connector_path(id)).await
}

pub async fn test_connector(id: &str) -> Result<ConnectorTest, ApiError> {
    let path = format!("{}/test", connector_path(id));
    client::request(Method::POST, &path, None).await
}
