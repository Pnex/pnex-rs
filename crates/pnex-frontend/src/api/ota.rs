//! OTA deployment endpoint — desired-state assignment creation.
//!
//! `POST /devices/{id}/ota` — create the desired-state assignment
//! (offline-safe: delivered at the next announce). Assignment state and
//! history reach the UI through the device list payload (`device.ota`),
//! not through a dedicated status fetch.

use crate::api::client;
use crate::api::error::ApiError;

/// `POST /api/v1/devices/{id}/ota` — returns the created assignment
/// (`pushed: true` = delivered to the online device, `false` = will be
/// delivered at the next announce).
pub async fn deploy(
    device_pk: i64,
    body: serde_json::Value,
) -> Result<serde_json::Value, ApiError> {
    client::request(
        reqwest::Method::POST,
        &format!("/api/v1/devices/{}/ota", device_pk),
        Some(body),
    )
    .await
}
