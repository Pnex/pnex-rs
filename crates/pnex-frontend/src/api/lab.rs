//! Vision Lab API (media-vision-studio.md §4.6, D198–D200):
//! `/api/v1/lab/…` — projects, images, annotations, dataset versions.

use pnex_core::lab::{
    LabAnnotationsInput, LabDatasetVersion, LabImage, LabProject, LabProjectInput,
    LabSuggestRequest, LabSuggestion,
};

use crate::api::client;
use crate::api::error::ApiError;

fn body<T: serde::Serialize>(input: &T) -> Option<serde_json::Value> {
    Some(serde_json::to_value(input).unwrap_or_default())
}

/// `GET /api/v1/lab/projects`.
pub async fn projects() -> Result<Vec<LabProject>, ApiError> {
    client::request(reqwest::Method::GET, "/api/v1/lab/projects", None).await
}

/// `GET /api/v1/lab/projects/{id}`.
pub async fn project(id: &str) -> Result<LabProject, ApiError> {
    client::request(
        reqwest::Method::GET,
        &format!("/api/v1/lab/projects/{id}"),
        None,
    )
    .await
}

/// `POST /api/v1/lab/projects`.
pub async fn create(input: &LabProjectInput) -> Result<LabProject, ApiError> {
    client::request(reqwest::Method::POST, "/api/v1/lab/projects", body(input)).await
}

/// `PUT /api/v1/lab/projects/{id}` — 409 `lab-class-in-use` when a removed
/// class still has boxes.
pub async fn update(id: &str, input: &LabProjectInput) -> Result<LabProject, ApiError> {
    client::request(
        reqwest::Method::PUT,
        &format!("/api/v1/lab/projects/{id}"),
        body(input),
    )
    .await
}

/// `DELETE /api/v1/lab/projects/{id}`.
pub async fn delete(id: &str) -> Result<Option<()>, ApiError> {
    client::request_opt(
        reqwest::Method::DELETE,
        &format!("/api/v1/lab/projects/{id}"),
        None,
    )
    .await
}

/// `GET /api/v1/lab/projects/{id}/images`.
pub async fn images(project: &str) -> Result<Vec<LabImage>, ApiError> {
    client::request(
        reqwest::Method::GET,
        &format!("/api/v1/lab/projects/{project}/images"),
        None,
    )
    .await
}

/// `POST /api/v1/lab/projects/{id}/images` — adds media library images
/// (one import = one source, the split unit).
pub async fn add_images(project: &str, asset_ids: &[String]) -> Result<Vec<LabImage>, ApiError> {
    client::request(
        reqwest::Method::POST,
        &format!("/api/v1/lab/projects/{project}/images"),
        Some(serde_json::json!({ "asset_ids": asset_ids })),
    )
    .await
}

/// `DELETE /api/v1/lab/images/{id}` — the media asset itself stays.
pub async fn remove_image(id: &str) -> Result<Option<()>, ApiError> {
    client::request_opt(
        reqwest::Method::DELETE,
        &format!("/api/v1/lab/images/{id}"),
        None,
    )
    .await
}

/// `PUT /api/v1/lab/images/{id}/annotations` — 409
/// `lab-annotations-conflict` when `expected_version` is stale.
pub async fn save_annotations(id: &str, input: &LabAnnotationsInput) -> Result<LabImage, ApiError> {
    client::request(
        reqwest::Method::PUT,
        &format!("/api/v1/lab/images/{id}/annotations"),
        body(input),
    )
    .await
}

/// `GET /api/v1/lab/projects/{id}/versions` — newest first.
pub async fn versions(project: &str) -> Result<Vec<LabDatasetVersion>, ApiError> {
    client::request(
        reqwest::Method::GET,
        &format!("/api/v1/lab/projects/{project}/versions"),
        None,
    )
    .await
}

/// `POST /api/v1/lab/projects/{id}/versions` — freezes the annotated
/// images into a new dataset version.
pub async fn freeze(project: &str, note: &str) -> Result<LabDatasetVersion, ApiError> {
    client::request(
        reqwest::Method::POST,
        &format!("/api/v1/lab/projects/{project}/versions"),
        Some(serde_json::json!({ "note": note })),
    )
    .await
}

/// `GET /api/v1/lab/versions/{id}/export` — COCO zip (images + one
/// annotation file per split).
pub async fn export(version: &str) -> Result<Vec<u8>, ApiError> {
    client::request_bytes(
        reqwest::Method::GET,
        &format!("/api/v1/lab/versions/{version}/export"),
    )
    .await
}

/// `POST /api/v1/lab/images/{id}/suggest` — boxes proposed by a registry
/// model (nothing is saved).
pub async fn suggest(id: &str, req: &LabSuggestRequest) -> Result<Vec<LabSuggestion>, ApiError> {
    client::request(
        reqwest::Method::POST,
        &format!("/api/v1/lab/images/{id}/suggest"),
        body(req),
    )
    .await
}

/// `POST /api/v1/lab/projects/{id}/import-recording` — frames of a camera
/// recording added as pictures (one source).
pub async fn import_recording(
    project: &str,
    req: &pnex_core::lab::LabRecordingImport,
) -> Result<Vec<LabImage>, ApiError> {
    client::request(
        reqwest::Method::POST,
        &format!("/api/v1/lab/projects/{project}/import-recording"),
        body(req),
    )
    .await
}

/// `POST /api/v1/lab/projects/{id}/import-video` — frames of a video of
/// the media library added as pictures.
pub async fn import_video(
    project: &str,
    req: &pnex_core::lab::LabVideoImport,
) -> Result<Vec<LabImage>, ApiError> {
    client::request(
        reqwest::Method::POST,
        &format!("/api/v1/lab/projects/{project}/import-video"),
        body(req),
    )
    .await
}

/// `POST /api/v1/lab/images/{id}/interpolate` — boxes interpolated between
/// the annotated frames around this one (nothing is saved).
pub async fn interpolate(id: &str) -> Result<Vec<LabSuggestion>, ApiError> {
    client::request(
        reqwest::Method::POST,
        &format!("/api/v1/lab/images/{id}/interpolate"),
        None,
    )
    .await
}

/// `POST /api/v1/lab/versions/{id}/train` — queues a training (D204).
pub async fn train(
    version: &str,
    req: &pnex_core::lab::LabTrainRequest,
) -> Result<pnex_core::lab::LabTrainingRun, ApiError> {
    client::request(
        reqwest::Method::POST,
        &format!("/api/v1/lab/versions/{version}/train"),
        body(req),
    )
    .await
}

/// `GET /api/v1/lab/projects/{id}/runs` — trainings, newest first.
pub async fn runs(project: &str) -> Result<Vec<pnex_core::lab::LabTrainingRun>, ApiError> {
    client::request(
        reqwest::Method::GET,
        &format!("/api/v1/lab/projects/{project}/runs"),
        None,
    )
    .await
}

/// `POST /api/v1/lab/runs/{id}/cancel`.
pub async fn cancel_run(id: &str) -> Result<pnex_core::lab::LabTrainingRun, ApiError> {
    client::request(
        reqwest::Method::POST,
        &format!("/api/v1/lab/runs/{id}/cancel"),
        None,
    )
    .await
}
