//! Vision model registry API (camera-video.md D81) — client of
//! `/api/v1/ml/models`: D14 list, create/update/delete, and the "test with
//! an image" endpoint (image bytes → detections).

use pnex_core::vision::{DetectionResult, LiveTestResult, MlModel, ModelInspection, ModelSpec};
use pnex_core::Paginated;
use serde::Serialize;

use crate::api::client;
use crate::api::error::ApiError;

/// D14 page of models.
pub type ModelPage = Paginated<MlModel>;

/// `GET /api/v1/ml/models` — D14 envelope.
pub async fn list(limit: Option<i64>, offset: Option<i64>) -> Result<ModelPage, ApiError> {
    let mut pairs = Vec::new();
    if let Some(limit) = limit {
        pairs.push(format!("limit={limit}"));
    }
    if let Some(offset) = offset {
        pairs.push(format!("offset={offset}"));
    }
    let query = if pairs.is_empty() {
        String::new()
    } else {
        format!("?{}", pairs.join("&"))
    };
    client::request(
        reqwest::Method::GET,
        &format!("/api/v1/ml/models{query}"),
        None,
    )
    .await
}

/// Body of `POST /api/v1/ml/models`.
#[derive(Debug, Clone, Serialize)]
pub struct CreateModel {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub asset_id: String,
    pub spec: ModelSpec,
}

/// `POST /api/v1/ml/models` — 400 body = field tokens.
pub async fn create(input: &CreateModel) -> Result<MlModel, ApiError> {
    client::request(
        reqwest::Method::POST,
        "/api/v1/ml/models",
        Some(serde_json::to_value(input).unwrap_or_default()),
    )
    .await
}

/// Body of `PATCH /api/v1/ml/models/{id}` (absent fields untouched).
#[derive(Debug, Clone, Default, Serialize)]
pub struct UpdateModel {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub spec: Option<ModelSpec>,
}

/// `PATCH /api/v1/ml/models/{id}`.
pub async fn update(id: &str, input: &UpdateModel) -> Result<MlModel, ApiError> {
    client::request(
        reqwest::Method::PATCH,
        &format!("/api/v1/ml/models/{id}"),
        Some(serde_json::to_value(input).unwrap_or_default()),
    )
    .await
}

/// `DELETE /api/v1/ml/models/{id}` — 204 (the ONNX media stays).
pub async fn delete(id: &str) -> Result<Option<()>, ApiError> {
    client::request_opt(
        reqwest::Method::DELETE,
        &format!("/api/v1/ml/models/{id}"),
        None,
    )
    .await
}

/// `POST /api/v1/ml/models/{id}/test` — JPEG/PNG bytes → detections.
pub async fn test_image(id: &str, image: Vec<u8>) -> Result<DetectionResult, ApiError> {
    client::request_upload(
        reqwest::Method::POST,
        &format!("/api/v1/ml/models/{id}/test"),
        image,
    )
    .await
}

/// `POST /api/v1/ml/models/{id}/check` — re-runs the D100 check, returns
/// the model with its stored status.
pub async fn check(id: &str) -> Result<MlModel, ApiError> {
    client::request(
        reqwest::Method::POST,
        &format!("/api/v1/ml/models/{id}/check"),
        None,
    )
    .await
}

/// `GET /api/v1/ml/models/inspect?asset_id=` — what an ONNX asset declares.
pub async fn inspect(asset_id: &str) -> Result<ModelInspection, ApiError> {
    client::request(
        reqwest::Method::GET,
        &format!("/api/v1/ml/models/inspect?asset_id={asset_id}"),
        None,
    )
    .await
}

/// `POST /api/v1/ml/models/{id}/test-live?device=` — one live round (D104).
pub async fn test_live(id: &str, device: i64) -> Result<LiveTestResult, ApiError> {
    client::request(
        reqwest::Method::POST,
        &format!("/api/v1/ml/models/{id}/test-live?device={device}"),
        None,
    )
    .await
}

/// Field errors of a 400 response: `(field, token)` pairs (school of
/// `api::cameras::field_errors`).
pub fn field_errors(err: &ApiError) -> Vec<(String, String)> {
    crate::api::cameras::field_errors(err)
}
