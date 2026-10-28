//! "stitch-jobs" API client — server-side 360° stitching (Take 360 V2).
//!
//! The app uploads the captured frames + poses to the backend, which runs the
//! Quality pipeline (alignment, DP seams, multi-band) at the server
//! `out_width` (4096 by default) and attaches the result as a **new
//! version** of the panorama asset (device preview = version 1). The media UI
//! follows the job state via [`get`] — polled "build_phase" model.

use serde::{Deserialize, Serialize};

use super::client;
use super::media::urlencode;

/// A pose at trigger time (degrees) — mirror of `capture360::guidance::Pose`.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct JobPose {
    pub yaw_deg: f32,
    pub pitch_deg: f32,
    pub roll_deg: f32,
}

/// Job creation payload.
#[derive(Debug, Serialize)]
pub struct CreateJob {
    pub asset_id: String,
    pub frames_total: usize,
    pub hfov_deg: f32,
    pub poses: Vec<JobPose>,
}

/// State of a server stitching job.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StitchJob {
    pub id: String,
    pub asset_id: String,
    pub state: String,
    pub error: Option<String>,
    pub frames_total: u32,
    pub frames_received: u32,
}

/// `POST /api/v1/stitch-jobs` — creates the job (poses + hfov), returns the state.
pub async fn create(req: &CreateJob) -> Result<StitchJob, crate::api::error::ApiError> {
    client::request(
        reqwest::Method::POST,
        "/api/v1/stitch-jobs",
        Some(
            serde_json::to_value(req)
                .map_err(|e| crate::api::error::ApiError::new(e.to_string()))?,
        ),
    )
    .await
}

/// `POST /api/v1/stitch-jobs/{id}/frames/{k}` — uploads frame k
/// (octet-stream, same mechanics as the media upload: 401 retry, org header).
pub async fn upload_frame(
    job_id: &str,
    k: usize,
    jpeg: Vec<u8>,
) -> Result<StitchJob, crate::api::error::ApiError> {
    client::request_upload(
        reqwest::Method::POST,
        &format!("/api/v1/stitch-jobs/{}/frames/{}", urlencode(job_id), k),
        jpeg,
    )
    .await
}

/// `GET /api/v1/stitch-jobs/{id}` — state (light UI poll).
pub async fn get(job_id: &str) -> Result<StitchJob, crate::api::error::ApiError> {
    client::request::<StitchJob>(
        reqwest::Method::GET,
        &format!("/api/v1/stitch-jobs/{}", urlencode(job_id)),
        None,
    )
    .await
}

/// `GET /api/v1/stitch-jobs/by-asset/{asset_id}` — latest job of an asset.
/// `Ok(None)` on 404: the asset was never stitched server-side (no HD
/// overlay in the media UI). Network/server errors bubble up as is.
pub async fn latest_for_asset(
    asset_id: &str,
) -> Result<Option<StitchJob>, crate::api::error::ApiError> {
    match client::request::<StitchJob>(
        reqwest::Method::GET,
        &format!("/api/v1/stitch-jobs/by-asset/{}", urlencode(asset_id)),
        None,
    )
    .await
    {
        Ok(job) => Ok(Some(job)),
        Err(err) if err.status == Some(404) => Ok(None),
        Err(err) => Err(err),
    }
}
