//! Client API « stitch-jobs » — assemblage 360° côté serveur (Take 360 V2).
//!
//! L'app téléverse les frames capturées + poses au backend, qui lance le
//! pipeline Quality (alignement, coutures DP, multi-bandes) à `out_width`
//! serveur (4096 par défaut) et attache le résultat comme **nouvelle
//! version** de l'asset panorama (aperçu device = version 1). L'UI médias
//! suit l'état du job via [`get`] — modèle « build_phase » pollé.

use serde::{Deserialize, Serialize};

use super::client;
use super::media::urlencode;

/// Une pose au déclenchement (degrés) — miroir de `capture360::guidance::Pose`.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct JobPose {
    pub yaw_deg: f32,
    pub pitch_deg: f32,
    pub roll_deg: f32,
}

/// Charge de création du job.
#[derive(Debug, Serialize)]
pub struct CreateJob {
    pub asset_id: String,
    pub frames_total: usize,
    pub hfov_deg: f32,
    pub poses: Vec<JobPose>,
}

/// État d'un job d'assemblage serveur.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StitchJob {
    pub id: String,
    pub asset_id: String,
    pub state: String,
    pub error: Option<String>,
    pub frames_total: u32,
    pub frames_received: u32,
}

/// `POST /api/v1/stitch-jobs` — crée le job (poses + hfov), renvoie l'état.
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

/// `POST /api/v1/stitch-jobs/{id}/frames/{k}` — téléverse la frame k
/// (octet-stream, même mécanique que l'upload média : retry 401, org header).
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

/// `GET /api/v1/stitch-jobs/{id}` — état (poll UI légère).
pub async fn get(job_id: &str) -> Result<StitchJob, crate::api::error::ApiError> {
    client::request::<StitchJob>(
        reqwest::Method::GET,
        &format!("/api/v1/stitch-jobs/{}", urlencode(job_id)),
        None,
    )
    .await
}

/// `GET /api/v1/stitch-jobs/by-asset/{asset_id}` — dernier job d'un asset.
/// `Ok(None)` sur 404 : l'asset n'a jamais été stitché serveur (pas d'overlay
/// HD dans l'UI médias). Erreurs réseau/serveur remontent telles quelles.
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
