//! Vision registry services (camera-video.md D81/D82): DTO mapping, model
//! bytes resolution through the media library, and a small in-process cache
//! of loaded detectors (tract plans are costly to optimize, cheap to run).
//!
//! Inference is CPU-bound: loading and detection always run on the blocking
//! pool, never on the async workers, and under the per-pod vision cap
//! (`compute_limits::vision`, `PNEX_VISION_MAX_CONCURRENT`).

use std::sync::{Arc, LazyLock, Mutex};

use loco_rs::prelude::*;
use pnex_core::vision::{
    MlModel, ModelCheck, ModelCheckStatus, ModelInspection, ModelSpec, VisionFamily, VisionTask,
};
use pnex_vision::Detector;
use sea_orm::{ColumnTrait, EntityTrait, QueryFilter};
use uuid::Uuid;

use crate::models::_entities::{media_assets, media_versions, ml_models};
use crate::services::compute_limits::{self, Saturated};
use crate::services::media::MediaSettings;

/// Failure of a capped blocking vision job.
#[derive(Debug)]
pub enum VisionError {
    /// The per-pod vision pool stayed full for the whole wait budget.
    Busy(Saturated),
    /// Loader/inference diagnostic (verbatim runtime text).
    Failed(String),
}

impl std::fmt::Display for VisionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Busy(s) => write!(f, "{s}"),
            Self::Failed(e) => f.write_str(e),
        }
    }
}

/// Runs a CPU-bound vision closure on the blocking pool, under the per-pod
/// vision concurrency cap.
pub async fn run_blocking<T, F>(f: F) -> Result<T, VisionError>
where
    F: FnOnce() -> T + Send + 'static,
    T: Send + 'static,
{
    let _permit = compute_limits::vision()
        .acquire()
        .await
        .map_err(VisionError::Busy)?;
    tokio::task::spawn_blocking(f)
        .await
        .map_err(|e| VisionError::Failed(e.to_string()))
}

/// Loaded detectors kept in memory (LRU-ish: oldest insertion evicted).
const CACHE_CAPACITY: usize = 4;

/// Cache key: model id + ONNX sha256 + spec fingerprint (a spec edit or a
/// new media version reloads).
type CacheKey = (Uuid, String);

static CACHE: LazyLock<Mutex<Vec<(CacheKey, Arc<Detector>)>>> =
    LazyLock::new(|| Mutex::new(Vec::new()));

pub fn spec_of(m: &ml_models::Model) -> ModelSpec {
    ModelSpec {
        family: VisionFamily::from_wire(&m.family).unwrap_or_default(),
        input_width: u32::try_from(m.input_width).unwrap_or(416),
        input_height: u32::try_from(m.input_height).unwrap_or(416),
        labels: serde_json::from_value(m.labels.clone()).unwrap_or_default(),
        score_threshold: m.score_threshold,
        nms_iou: m.nms_iou,
    }
}

pub fn dto(m: &ml_models::Model) -> MlModel {
    MlModel {
        id: m.id.to_string(),
        name: m.name.clone(),
        description: m.description.clone().unwrap_or_default(),
        task: VisionTask::Detection,
        asset_id: m.asset_id.to_string(),
        asset_version: m.asset_version,
        spec: spec_of(m),
        check: check_of(m),
        created_at: m.created_at.to_rfc3339(),
        updated_at: m.updated_at.to_rfc3339(),
    }
}

/// Stored D100 check of a model row.
pub fn check_of(m: &ml_models::Model) -> ModelCheck {
    ModelCheck {
        status: ModelCheckStatus::from_wire(&m.check_status),
        error: m.check_error.clone(),
        infer_ms: m.infer_ms.and_then(|v| u64::try_from(v).ok()),
        checked_at: m.checked_at.map(|t| t.to_rfc3339()),
    }
}

/// Media version holding the model bytes (pinned, else current).
pub async fn model_version(
    db: &DatabaseConnection,
    m: &ml_models::Model,
) -> Result<Option<media_versions::Model>> {
    asset_version_row(db, m.asset_id, m.asset_version).await
}

/// Media version of an asset: `version` pinned, else the current one.
pub async fn asset_version_row(
    db: &DatabaseConnection,
    asset_id: Uuid,
    version: Option<i64>,
) -> Result<Option<media_versions::Model>> {
    let q = media_versions::Entity::find().filter(media_versions::Column::AssetId.eq(asset_id));
    let found = match version {
        Some(n) => {
            q.filter(media_versions::Column::VersionNumber.eq(n))
                .one(db)
                .await
        }
        None => {
            let Some(asset) = media_assets::Entity::find_by_id(asset_id)
                .one(db)
                .await
                .map_err(|_| Error::InternalServerError)?
            else {
                return Ok(None);
            };
            let Some(current) = asset.current_version_id else {
                return Ok(None);
            };
            media_versions::Entity::find_by_id(current).one(db).await
        }
    };
    found.map_err(|_| Error::InternalServerError)
}

/// ONNX bytes + sha of a model.
pub async fn model_bytes(
    ctx: &AppContext,
    m: &ml_models::Model,
) -> Result<Option<(Vec<u8>, String)>> {
    asset_bytes(ctx, m.asset_id, m.asset_version).await
}

/// ONNX bytes + sha of an asset version (pinned, else current).
pub async fn asset_bytes(
    ctx: &AppContext,
    asset_id: Uuid,
    version: Option<i64>,
) -> Result<Option<(Vec<u8>, String)>> {
    let Some(version) = asset_version_row(&ctx.db, asset_id, version).await? else {
        return Ok(None);
    };
    let store = MediaSettings::from_config(&ctx.config)
        .store()
        .map_err(|_| Error::InternalServerError)?;
    let bytes = store
        .get(&version.storage_key)
        .await
        .map_err(|_| Error::NotFound)?;
    let sha = version
        .sha256
        .clone()
        .unwrap_or_else(|| version.id.to_string());
    Ok(Some((bytes, sha)))
}

fn cache_key(m: &ml_models::Model, sha: &str) -> CacheKey {
    let spec = serde_json::to_string(&spec_of(m)).unwrap_or_default();
    (m.id, format!("{sha}:{spec}"))
}

/// Loaded detector of a model (cached). `Ok(None)` = no model bytes.
pub async fn detector(
    ctx: &AppContext,
    m: &ml_models::Model,
) -> Result<Option<Arc<Detector>>, String> {
    let Some(version) = model_version(&ctx.db, m).await.map_err(|e| e.to_string())? else {
        return Ok(None);
    };
    let sha = version
        .sha256
        .clone()
        .unwrap_or_else(|| version.id.to_string());
    let key = cache_key(m, &sha);
    if let Some(hit) = CACHE
        .lock()
        .expect("vision cache")
        .iter()
        .find(|(k, _)| *k == key)
        .map(|(_, d)| d.clone())
    {
        return Ok(Some(hit));
    }
    let Some((bytes, _)) = model_bytes(ctx, m).await.map_err(|e| e.to_string())? else {
        return Ok(None);
    };
    let spec = spec_of(m);
    let det = run_blocking(move || Detector::load(&bytes, spec))
        .await
        .map_err(|e| e.to_string())?
        .map_err(|e| e.to_string())?;
    let det = Arc::new(det);
    let mut cache = CACHE.lock().expect("vision cache");
    cache.retain(|(k, _)| k.0 != m.id);
    if cache.len() >= CACHE_CAPACITY {
        cache.remove(0);
    }
    cache.push((key, det.clone()));
    Ok(Some(det))
}

/// Drops the cached detectors of a model (after update/delete).
pub fn evict(model_id: Uuid) {
    CACHE
        .lock()
        .expect("vision cache")
        .retain(|(k, _)| k.0 != model_id);
}

/// Runs a detection on the blocking pool.
pub async fn detect(
    det: Arc<Detector>,
    image: Vec<u8>,
) -> Result<pnex_core::vision::DetectionResult, VisionError> {
    run_blocking(move || det.detect(&image))
        .await?
        .map_err(|e| VisionError::Failed(e.to_string()))
}

/// Why a spec cannot be used with a model file (D100).
#[derive(Debug, Clone, PartialEq)]
pub enum SpecProblem {
    /// Field-level machine token (`labels` → `label_count:80`).
    Field(&'static str, String),
    /// Loader/inference diagnostic (verbatim runtime text).
    Load(String),
    /// The per-pod vision pool is saturated (503 server-busy).
    Busy(Saturated),
}

/// Reads what an ONNX file declares (blocking pool).
pub async fn inspect_bytes(bytes: Vec<u8>) -> Result<ModelInspection, VisionError> {
    run_blocking(move || pnex_vision::inspect(&bytes))
        .await?
        .map_err(|e| VisionError::Failed(e.to_string()))
}

/// Aligns `spec` on what the file dictates, then proves the pair works:
/// fixed input dimensions are imposed (never user-entered), the label count
/// must match the output head, and the model must load and run one
/// inference. Returns the effective spec and the measured inference time.
pub async fn prepare_spec(
    bytes: Vec<u8>,
    mut spec: ModelSpec,
) -> Result<(ModelSpec, u64), SpecProblem> {
    run_blocking(move || {
        let info = pnex_vision::inspect(&bytes).map_err(|e| SpecProblem::Load(e.to_string()))?;
        if let Some((w, h)) = info.fixed_input() {
            spec.input_width = w;
            spec.input_height = h;
        }
        if let Some(classes) = info.classes {
            if classes as usize != spec.labels.len() {
                return Err(SpecProblem::Field(
                    "labels",
                    format!("label_count:{classes}"),
                ));
            }
        }
        let v = pnex_vision::validate(&bytes, spec.clone())
            .map_err(|e| SpecProblem::Load(e.to_string()))?;
        Ok((spec, v.infer_ms))
    })
    .await
    .map_err(|e| match e {
        VisionError::Busy(s) => SpecProblem::Busy(s),
        VisionError::Failed(e) => SpecProblem::Load(e),
    })?
}

/// Re-checks a stored model as is (no spec rewrite) and persists the
/// outcome — the "Check" button and the pre-D100 rows.
pub async fn check_model(ctx: &AppContext, m: ml_models::Model) -> Result<ml_models::Model> {
    use sea_orm::{ActiveModelTrait, Set};
    let outcome = match model_bytes(ctx, &m).await? {
        None => Err("the model file is missing from the media library".to_string()),
        Some((bytes, _)) => {
            let spec = spec_of(&m);
            match run_blocking(move || pnex_vision::validate(&bytes, spec)).await {
                // Saturation is not a verdict on the model: never persist it.
                Err(VisionError::Busy(s)) => return Err(compute_limits::busy_error(s)),
                Err(VisionError::Failed(e)) => Err(e),
                Ok(r) => r.map_err(|e| e.to_string()),
            }
        }
    };
    let mut upd: ml_models::ActiveModel = m.into();
    match outcome {
        Ok(v) => {
            upd.check_status = Set(ModelCheckStatus::Valid.wire().into());
            upd.check_error = Set(None);
            upd.infer_ms = Set(Some(v.infer_ms as i64));
        }
        Err(e) => {
            upd.check_status = Set(ModelCheckStatus::Invalid.wire().into());
            upd.check_error = Set(Some(e));
        }
    }
    upd.checked_at = Set(Some(chrono::Utc::now().into()));
    let saved = upd
        .update(&ctx.db)
        .await
        .map_err(|_| Error::InternalServerError)?;
    evict(saved.id);
    Ok(saved)
}

#[cfg(test)]
mod tests {
    #[test]
    fn detector_is_shareable() {
        fn check<T: Send + Sync>() {}
        check::<pnex_vision::Detector>();
    }
}
