//! Voice identification (voice-id.md): consented profiles of Person
//! objects, their enrolment clips, generation and tests. Never a vector.

use pnex_core::voice_id::{
    VoiceProfileInput, VoiceProfilePatch, VoiceProfileView, VoiceTestResult,
};

use super::client;
use super::media::urlencode;
use crate::api::error::ApiError;

/// `GET /api/v1/voice-id/profiles`, all pages.
pub async fn list() -> Result<Vec<VoiceProfileView>, ApiError> {
    client::request_all("/api/v1/voice-id/profiles").await
}

/// `GET /api/v1/voice-id/by-object/{id}` — `None` when the person has none.
pub async fn by_object(object_id: &str) -> Result<Option<VoiceProfileView>, ApiError> {
    client::request(
        reqwest::Method::GET,
        &format!("/api/v1/voice-id/by-object/{object_id}"),
        None,
    )
    .await
}

/// `POST /api/v1/voice-id/profiles` — consent mandatory.
pub async fn create(input: &VoiceProfileInput) -> Result<VoiceProfileView, ApiError> {
    client::request(
        reqwest::Method::POST,
        "/api/v1/voice-id/profiles",
        serde_json::to_value(input).ok(),
    )
    .await
}

/// `PATCH /api/v1/voice-id/profiles/{id}` — other embedding model.
pub async fn set_model(id: &str, model_id: &str) -> Result<VoiceProfileView, ApiError> {
    let patch = VoiceProfilePatch {
        model_id: model_id.to_string(),
    };
    client::request(
        reqwest::Method::PATCH,
        &format!("/api/v1/voice-id/profiles/{id}"),
        serde_json::to_value(&patch).ok(),
    )
    .await
}

/// `DELETE /api/v1/voice-id/profiles/{id}` — consent revoked.
pub async fn revoke(id: &str) -> Result<Option<()>, ApiError> {
    client::request_opt(
        reqwest::Method::DELETE,
        &format!("/api/v1/voice-id/profiles/{id}"),
        None,
    )
    .await
}

/// `POST /api/v1/voice-id/profiles/{id}/samples` — one enrolment clip,
/// `duration` seconds from `start` into the recording.
pub async fn add_sample(
    id: &str,
    file_name: &str,
    content_type: &str,
    bytes: Vec<u8>,
    start: u32,
    duration: u32,
) -> Result<VoiceProfileView, ApiError> {
    client::request_upload(
        reqwest::Method::POST,
        &format!(
            "/api/v1/voice-id/profiles/{id}/samples?name={}&content_type={}&start={start}&duration={duration}",
            urlencode(file_name),
            urlencode(content_type)
        ),
        bytes,
    )
    .await
}

/// `DELETE /api/v1/voice-id/profiles/{id}/samples/{sample_id}`.
pub async fn remove_sample(id: &str, sample_id: &str) -> Result<Option<()>, ApiError> {
    client::request_opt(
        reqwest::Method::DELETE,
        &format!("/api/v1/voice-id/profiles/{id}/samples/{sample_id}"),
        None,
    )
    .await
}

/// `POST /api/v1/voice-id/profiles/{id}/generate`.
pub async fn generate(id: &str) -> Result<VoiceProfileView, ApiError> {
    client::request(
        reqwest::Method::POST,
        &format!("/api/v1/voice-id/profiles/{id}/generate"),
        None,
    )
    .await
}

/// `POST /api/v1/voice-id/test` — `object_id` = verification of one person,
/// else identification among the ready profiles of `model_id`.
pub async fn test(
    object_id: Option<&str>,
    model_id: Option<&str>,
    file_name: &str,
    content_type: &str,
    bytes: Vec<u8>,
) -> Result<VoiceTestResult, ApiError> {
    let mut path = format!(
        "/api/v1/voice-id/test?name={}&content_type={}",
        urlencode(file_name),
        urlencode(content_type)
    );
    if let Some(o) = object_id {
        path.push_str(&format!("&object_id={}", urlencode(o)));
    }
    if let Some(m) = model_id.filter(|m| !m.is_empty()) {
        path.push_str(&format!("&model_id={}", urlencode(m)));
    }
    client::request_upload(reqwest::Method::POST, &path, bytes).await
}
