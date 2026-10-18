//! Objects detected on a video of the library (media-import-pipeline.md
//! D215, media-review.md R5): pictures decoded by the confined decoder
//! (D203) one every [`EVERY_SECS`], run through a detection model of the
//! org, boxes written to O2 `camera_detections` like the camera layers
//! (D105) under the source key `media:<asset>`. Boxes only, never pixels.
//!
//! The documents are dated `run date + position` (O2 takes recent
//! documents only); the run date and the layer are kept in the analysis
//! `result_ref`, the reader turns the dates back into positions.

use loco_rs::app::AppContext;
use pnex_core::camera::{AnnotationBatch, FrameAnnotation, ANNOTATIONS_MAX};
use pnex_core::err_codes;
use pnex_core::media_review::MediaDetections;
use pnex_core::vision::ModelCheckStatus;
use sea_orm::{ColumnTrait, EntityTrait, QueryFilter, QueryOrder};
use uuid::Uuid;

use crate::models::_entities::{media_versions, ml_models};
use crate::services::media::MediaSettings;
use crate::services::{video_annotations, vision};

/// Seconds between two analysed pictures.
pub const EVERY_SECS: f64 = 2.0;
/// Pictures decoded per decoder run (the decoder's own bound).
const CHUNK: usize = 200;
/// Longest video analysed (4 h, D211 bound).
const MAX_SECS: f64 = 4.0 * 3600.0;
/// Largest file read in memory.
const MAX_BYTES: i64 = 512 * 1024 * 1024;

/// Source key of the detections of an asset in `camera_detections`.
pub fn source_of(asset: Uuid) -> String {
    format!("media:{asset}")
}

/// First valid object detection model of the org (by name).
// shortcut: no model picker on the analysis, the first valid detector is
// taken; add a choice when an org registers several.
pub async fn detector_model(
    db: &sea_orm::DatabaseConnection,
    org_id: i64,
) -> Option<ml_models::Model> {
    ml_models::Entity::find()
        .filter(ml_models::Column::OrgId.eq(org_id))
        .filter(ml_models::Column::Task.eq(vision::VISION_TASK))
        .filter(ml_models::Column::CheckStatus.eq(ModelCheckStatus::Valid.wire()))
        .order_by_asc(ml_models::Column::Name)
        .one(db)
        .await
        .ok()
        .flatten()
}

/// Detects objects on `version_id` for analysis `analysis_id`; returns the
/// `result_ref` (`layer_id`, `base_ms`, `model`, `frames`).
pub async fn run(
    ctx: &AppContext,
    org_id: i64,
    version_id: Uuid,
    analysis_id: Uuid,
) -> Result<serde_json::Value, &'static str> {
    let failed = err_codes::MEDIA_ANALYSIS_DETECT_FAILED;
    let version = media_versions::Entity::find_by_id(version_id)
        .filter(media_versions::Column::OrgId.eq(org_id))
        .one(&ctx.db)
        .await
        .ok()
        .flatten()
        .ok_or(failed)?;
    let format =
        pnex_media_capture::decoder::video_file_format(&version.content_type).ok_or(failed)?;
    if version.size_bytes > MAX_BYTES {
        return Err(failed);
    }
    let model = detector_model(&ctx.db, org_id)
        .await
        .ok_or(err_codes::MEDIA_ANALYSIS_NO_DETECTOR)?;
    let det = vision::detector(ctx, &model)
        .await
        .ok()
        .flatten()
        .ok_or(err_codes::MEDIA_ANALYSIS_NO_DETECTOR)?;
    // One video decode at a time per process, shared with the Vision Lab.
    let _gate = crate::services::lab::VIDEO_IMPORTS
        .acquire()
        .await
        .map_err(|_| failed)?;
    let bytes = MediaSettings::from_config(&ctx.config)
        .store()
        .map_err(|_| failed)?
        .get(&version.storage_key)
        .await
        .map_err(|_| failed)?;
    let layer_id = format!("a{}", analysis_id.simple());
    let base_ms = chrono::Utc::now().timestamp_millis();
    let every_ms = (EVERY_SECS * 1000.0) as i64;
    let mut from = 0.0f64;
    let mut frames = 0usize;
    while from < MAX_SECS {
        let jpegs =
            pnex_media_capture::decode_video_frames(bytes.clone(), format, from, EVERY_SECS, CHUNK)
                .await;
        let jpegs = match jpegs {
            Ok(j) => j,
            // Past the end of the video: no picture left.
            Err(pnex_media_capture::CaptureError::FormatUnsupported) if frames > 0 => break,
            Err(e) => {
                tracing::warn!(version = %version.id, error = e.code(), "video pictures not decoded");
                return Err(failed);
            }
        };
        let n = jpegs.len();
        let start_ms = (from * 1000.0) as i64;
        let mut annotations = Vec::with_capacity(n);
        for (k, jpeg) in jpegs.into_iter().enumerate() {
            let result = loop {
                match vision::detect(det.clone(), jpeg.bytes.clone()).await {
                    Err(vision::VisionError::Busy(_)) => {
                        tokio::time::sleep(std::time::Duration::from_millis(500)).await
                    }
                    other => break other,
                }
            };
            let result = result.map_err(|e| {
                tracing::warn!(version = %version.id, error = %e, "picture not analysed");
                failed
            })?;
            annotations.push(FrameAnnotation {
                layer_id: layer_id.clone(),
                ts_ms: base_ms + start_ms + k as i64 * every_ms,
                width: result.width,
                height: result.height,
                detections: result.detections,
            });
        }
        frames += n;
        let batch = AnnotationBatch {
            org_id,
            device_id: source_of(version.asset_id),
            layer_id: layer_id.clone(),
            layer: model.name.clone(),
            flow_id: None,
            annotations,
        };
        video_annotations::record(ctx, &batch).await.map_err(|e| {
            tracing::warn!(version = %version.id, error = %e, "detections not stored");
            failed
        })?;
        if n < CHUNK {
            break;
        }
        from += CHUNK as f64 * EVERY_SECS;
    }
    Ok(serde_json::json!({
        "layer_id": layer_id,
        "base_ms": base_ms,
        "model": model.name,
        "frames": frames,
    }))
}

/// Detections of the latest finished detection of an asset's current
/// version, positions in the media.
pub async fn read(
    ctx: &AppContext,
    org_id: i64,
    asset_id: Uuid,
    result_ref: Option<serde_json::Value>,
) -> MediaDetections {
    let mut out = MediaDetections {
        model: None,
        frames: Vec::new(),
        unavailable: false,
    };
    let Some(r) = result_ref else {
        return out;
    };
    let (Some(layer), Some(base_ms)) = (r["layer_id"].as_str(), r["base_ms"].as_i64()) else {
        return out;
    };
    out.model = r["model"].as_str().map(str::to_string);
    let to_ms = base_ms + (MAX_SECS as i64 + 3600) * 1000;
    match video_annotations::search(
        ctx,
        org_id,
        &source_of(asset_id),
        &[layer.to_string()],
        base_ms - 60_000,
        to_ms,
    )
    .await
    {
        Ok(frames) => {
            out.frames = frames
                .into_iter()
                .take(ANNOTATIONS_MAX as usize)
                .map(|mut f| {
                    f.ts_ms -= base_ms;
                    f
                })
                .collect();
        }
        Err(e) => {
            tracing::warn!(asset = %asset_id, error = %e, "detections not read");
            out.unavailable = true;
        }
    }
    out
}
