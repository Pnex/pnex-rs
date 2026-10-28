//! Take 360 V2 — server stitch jobs (plan cheerful-booping-teacup B2):
//! the app uploads a ring of frames + poses, the server assembles the
//! HD panorama (worker `stitch_panorama`) and publishes it as version n+1 of
//! the media asset. The device keeps its immediate 1024 preview (v1); the
//! job progress (`frames_received/frames_total` then `state`) is
//! polled by the front — "build_phase" model.
//!
//! Frame upload: **raw octet-stream bytes** (never multipart —
//! media.rs pattern), one request per frame, dedicated `DefaultBodyLimit` per
//! route. The bytes live in the MediaStore (same settings as
//! media — `settings.media`), never in the DB.
//!
//! Frame deduplication deliberately ABSENT: each POST increments
//! `frames_received` (same frame resent → wrong count). Client contract:
//! the app never uploads the same frame twice (retry = new frame k
//! overwritten in the store, counter to bump client-side). Accepted deviation
//! from the "clean" bitmask: the dedup column was not worth the
//! complexity for a client that controls its stream (plan B2).
//!
//! Errors (D239, all `ErrorDetail`): 400 `field-invalid` with machine
//! tokens, 403 `access-denied`, 404 masking other orgs, 409 on a wrong
//! state, 503 `stitch-disabled` when server stitching is off.

use axum::body::Bytes;
use axum::extract::{DefaultBodyLimit, Path, State};
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::Json;
use loco_rs::prelude::*;
use sea_orm::{
    ActiveModelTrait, ActiveValue::Set, ColumnTrait, EntityTrait, QueryFilter, QueryOrder,
};
use serde::Deserialize;
use uuid::Uuid;

use crate::auth::OrgContext;
use crate::models::_entities::{media_assets, stitch_jobs};
use crate::services::media::MediaSettings;
use crate::services::stitch::{
    StitchSettings, STATE_FAILED, STATE_QUEUED, STATE_RUNNING, STATE_SUCCEEDED,
};

use crate::workers::stitch_panorama::{PoseDeg, StitchPanoramaArgs, StitchPanoramaWorker};

/// Cap of an uploaded frame — 8 MB (1080×1920 JPEG q85 frame ≈ 400-700
/// KB: wide margin). The axum limit, set to max+1 MB on the route, stays
/// the safety net (media.rs::routes pattern).
pub const FRAME_MAX_BYTES: usize = 8 * 1024 * 1024;

/// Job bounds: a 360° ring = up to 64 frames (plan C1: 28 in
/// real use), no single pose (the stitcher needs overlap).
const MAX_FRAMES: u32 = 64;

// ─────────────────────────── Aides ───────────────────────────

/// 400 `field-invalid` with one machine token.
fn field_status(field: &str, msg: &str) -> Response {
    super::field_error(field, msg)
}

/// 409 — POST frame on a terminal job (succeeded/failed). Carries the
/// machine code + canonical English description (frontend resolves
/// `err-<code>`, verbatim fallback otherwise).
fn conflict(code: &str, msg: &str) -> Error {
    Error::CustomError(
        StatusCode::CONFLICT,
        loco_rs::controller::ErrorDetail::new(code, msg.to_string()),
    )
}

/// 503 — kill-switch `settings.stitch.enabled=false`.
fn disabled() -> Response {
    super::coded_error(
        StatusCode::SERVICE_UNAVAILABLE,
        pnex_core::err_codes::STITCH_DISABLED,
        "Server-side stitching is disabled.",
        None,
    )
    .into_response()
}

/// Job of the current org, otherwise None (→ 404 masking cross-org,
/// `device_of_org` / media.rs::find_asset pattern).
async fn find_job(
    db: &DatabaseConnection,
    org: &OrgContext,
    id: Uuid,
) -> Result<Option<stitch_jobs::Model>> {
    stitch_jobs::Entity::find_by_id(id)
        .filter(stitch_jobs::Column::OrgId.eq(org.org.id))
        .one(db)
        .await
        .map_err(|_| Error::InternalServerError)
}

/// Asset of the current org (checked before job creation — the job
/// references an existing asset, never created on the fly).
async fn find_asset(
    db: &DatabaseConnection,
    org: &OrgContext,
    id: Uuid,
) -> Result<Option<media_assets::Model>> {
    media_assets::Entity::find_by_id(id)
        .filter(media_assets::Column::OrgId.eq(org.org.id))
        .one(db)
        .await
        .map_err(|_| Error::InternalServerError)
}

// ─────────────────────────── DTOs ───────────────────────────

/// State of a stitch job (stored as its wire string).
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize, utoipa::ToSchema,
)]
#[serde(rename_all = "snake_case")]
enum StitchState {
    Queued,
    Running,
    Succeeded,
    Failed,
}

impl StitchState {
    fn parse(raw: &str) -> Self {
        match raw {
            STATE_QUEUED => Self::Queued,
            STATE_RUNNING => Self::Running,
            STATE_SUCCEEDED => Self::Succeeded,
            // Unknown rows are reported as failed (defence in depth).
            _ => Self::Failed,
        }
    }
}

/// A server-side panorama stitch job.
#[derive(serde::Serialize, utoipa::ToSchema)]
struct StitchJobDto {
    #[schema(value_type = String, format = "uuid")]
    id: Uuid,
    #[schema(value_type = String, format = "uuid")]
    asset_id: Uuid,
    state: StitchState,
    error: Option<String>,
    frames_total: i32,
    frames_received: i32,
}

fn job_dto(j: &stitch_jobs::Model) -> StitchJobDto {
    StitchJobDto {
        id: j.id,
        asset_id: j.asset_id,
        state: StitchState::parse(&j.state),
        error: j.error.clone(),
        frames_total: j.frames_total,
        frames_received: j.frames_received,
    }
}

// ─────────────────────── POST /stitch-jobs ───────────────────────

#[derive(Deserialize, utoipa::ToSchema)]
struct CreateJob {
    /// Media asset the panorama is published to (as a new version).
    #[schema(value_type = String, format = "uuid")]
    asset_id: Uuid,
    /// Number of frames that will be uploaded (2 to 64).
    frames_total: u32,
    /// Horizontal field of view of the frames, degrees (1 to 179).
    hfov_deg: f32,
    /// One pose per frame, in upload order.
    poses: Vec<PoseDeg>,
}

/// `POST /api/v1/stitch-jobs` — creates the `queued` job (poses + hfov, ~2 KB).
/// The frames then go one by one; the last one triggers the worker.
#[utoipa::path(
    post,
    path = "/api/v1/stitch-jobs",
    tag = "stitch-jobs",
    operation_id = "stitch_jobs_create",
    summary = "Create a panorama stitch job",
    description = "Creates a `queued` job for a 360 capture ring (poses + field of view). Frames are then uploaded one by one with `POST /api/v1/stitch-jobs/{id}/frames/{k}`; the last one starts the server stitch, whose panorama is published as a new version of the media asset. Writer role required.",
    request_body = CreateJob,
    responses(
        (status = 201, body = StitchJobDto),
        (status = 400, description = "`field-invalid`: `frames_total` `range:2..64`, `poses` `invalid` (one pose per frame), `hfov_deg` `range:1..179`", body = crate::openapi::ErrorDetail),
        (status = 404, description = "Unknown media asset", body = crate::openapi::ErrorDetail),
        (status = 503, description = "Server-side stitching disabled (`stitch-disabled`)", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn create(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Json(body): Json<CreateJob>,
) -> Result<Response> {
    let stitch_settings = StitchSettings::from_config(&ctx.config);
    if !stitch_settings.enabled {
        return Ok(disabled());
    }
    // Validation (plan B2): 2 ≤ poses == frames_total ≤ 64, plausible hfov
    // (stitcher bounds: 1..=179°).
    if body.frames_total < 2 || body.frames_total > MAX_FRAMES {
        return Ok(field_status(
            "frames_total",
            &format!("range:2..{MAX_FRAMES}"),
        ));
    }
    if body.poses.len() != body.frames_total as usize {
        // One pose per frame: poses.len() == frames_total.
        return Ok(field_status("poses", pnex_core::err_codes::FIELD_INVALID));
    }
    if !(1.0..=179.0).contains(&body.hfov_deg) {
        return Ok(field_status("hfov_deg", "range:1..179"));
    }
    if find_asset(&ctx.db, &org, body.asset_id).await?.is_none() {
        return Err(Error::NotFound);
    }
    let job = stitch_jobs::ActiveModel {
        id: Set(Uuid::new_v4()),
        org_id: Set(org.org.id),
        asset_id: Set(body.asset_id),
        state: Set(STATE_QUEUED.to_string()),
        error: Set(None),
        frames_total: Set(body.frames_total as i32),
        frames_received: Set(0),
        poses_json: Set(serde_json::to_value(&body.poses).unwrap_or_default()),
        hfov_deg: Set(body.hfov_deg),
        ..Default::default()
    }
    .insert(&ctx.db)
    .await
    .map_err(|_| Error::InternalServerError)?;
    Ok((StatusCode::CREATED, format::json(job_dto(&job))).into_response())
}

// ─────────────── POST /stitch-jobs/{id}/frames/{k} ───────────────

#[derive(Deserialize)]
struct FramePath {
    id: Uuid,
    k: u32,
}

/// `POST /api/v1/stitch-jobs/{id}/frames/{k}` — JPEG octet-stream of a
/// frame. Increments `frames_received` (without deduplicating, see module
/// doc); the last received frame chains the worker **within the request**
/// (ForegroundBlocking in test, PG queue in dev/prod).
#[utoipa::path(
    post,
    path = "/api/v1/stitch-jobs/{id}/frames/{k}",
    tag = "stitch-jobs",
    operation_id = "stitch_jobs_frames_upload",
    summary = "Upload one frame of a stitch job",
    description = "Raw JPEG bytes of frame `k` (max 8 MB). Each call increments `frames_received` (no de-duplication: never send a frame twice); the last frame queues the stitch worker (compute-heavy). Writer role required.",
    params(("id" = String, Path, format = "uuid", description = "Stitch job UUID"), ("k" = u32, Path, description = "Frame index, 0-based (< frames_total)")),
    request_body(content = Vec<u8>, content_type = "application/octet-stream", description = "JPEG frame bytes"),
    responses(
        (status = 200, body = StitchJobDto),
        (status = 400, description = "`field-invalid` on `k`: `invalid` (index out of bounds) or `required` (empty body)", body = crate::openapi::ErrorDetail),
        (status = 404, description = "Unknown job", body = crate::openapi::ErrorDetail),
        (status = 409, description = "Job already finished (`stitch-job-terminal`) or in an unknown state (`stitch-state-unknown`)", body = crate::openapi::ErrorDetail),
        (status = 413, description = "Frame too large (`stitch-frame-too-large`, `errors.args.limit`)", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn upload_frame(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(path): Path<FramePath>,
    body: Bytes,
) -> Result<Response> {
    let Some(job) = find_job(&ctx.db, &org, path.id).await? else {
        return Err(Error::NotFound);
    };
    // State machine: a terminal job (succeeded/failed) accepts nothing
    // anymore (409) — no silent mutation of an already published result.
    // Any other unknown state is rejected the same way (defense in depth).
    match job.state.as_str() {
        STATE_QUEUED | STATE_RUNNING => {}
        STATE_SUCCEEDED | STATE_FAILED => {
            return Err(conflict(
                "stitch-job-terminal",
                "Job is already terminal — create a new job to re-stitch.",
            ));
        }
        _ => return Err(conflict("stitch-state-unknown", "Unknown job state.")),
    }
    if path.k >= job.frames_total as u32 {
        return Ok(field_status("k", pnex_core::err_codes::FIELD_INVALID));
    }
    if body.is_empty() {
        return Ok(field_status("k", pnex_core::err_codes::FIELD_REQUIRED));
    }
    if body.len() > FRAME_MAX_BYTES {
        return Ok(super::coded_error(
            StatusCode::PAYLOAD_TOO_LARGE,
            pnex_core::err_codes::STITCH_FRAME_TOO_LARGE,
            "Frame too large.",
            Some(serde_json::json!({ "limit": FRAME_MAX_BYTES.to_string() })),
        )
        .into_response());
    }

    // Bytes → MediaStore (first, the DB afterwards — write_version pattern:
    // an orphan blob can be purged, a lying row cannot).
    let media = MediaSettings::from_config(&ctx.config);
    let store = media.store().map_err(|_| Error::InternalServerError)?;
    let key = MediaSettings::stitch_key(job.id, path.k);
    store
        .put(&key, body)
        .await
        .map_err(|_| Error::InternalServerError)?;

    // Last frame → worker enqueue (ForegroundBlocking in test:
    // inline execution, the job is already terminal when this request
    // responds).
    let frames_received = job.frames_received + 1;
    let enqueue = frames_received == job.frames_total;
    let mut job: stitch_jobs::ActiveModel = job.into();
    job.frames_received = Set(frames_received);
    job.updated_at = Set(chrono::Utc::now().into());
    let job = job
        .update(&ctx.db)
        .await
        .map_err(|_| Error::InternalServerError)?;
    if enqueue {
        StitchPanoramaWorker::perform_later(
            &ctx,
            StitchPanoramaArgs {
                job_id: job.id,
                org_id: org.org.id,
            },
        )
        .await
        .map_err(|_| Error::InternalServerError)?;
        // Re-read: in ForegroundBlocking (tests, saturated single worker), the
        // worker already ran — the response carries the terminal state, not a
        // stale state.
        let job = stitch_jobs::Entity::find_by_id(job.id)
            .one(&ctx.db)
            .await
            .map_err(|_| Error::InternalServerError)?
            .ok_or(Error::NotFound)?;
        return Ok(format::json(job_dto(&job)).into_response());
    }
    Ok(format::json(job_dto(&job)).into_response())
}

// ─────────────────────── GET /stitch-jobs/{id} ───────────────────────

/// `GET /api/v1/stitch-jobs/{id}` — job state (UI poll: upload progress
/// then assembly).
#[utoipa::path(
    get,
    path = "/api/v1/stitch-jobs/{id}",
    tag = "stitch-jobs",
    operation_id = "stitch_jobs_get",
    summary = "Get a stitch job",
    description = "Job state and upload progress (`frames_received` / `frames_total`), polled by the UI.",
    params(("id" = String, Path, format = "uuid", description = "Stitch job UUID")),
    responses(
        (status = 200, body = StitchJobDto),
        (status = 404, description = "Unknown job or job of another organisation", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn detail(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(id): Path<Uuid>,
) -> Result<Response> {
    let Some(job) = find_job(&ctx.db, &org, id).await? else {
        return Err(Error::NotFound);
    };
    Ok(format::json(job_dto(&job)).into_response())
}

// ───────────────── GET /stitch-jobs/by-asset/{asset_id} ─────────────────

/// `GET /api/v1/stitch-jobs/by-asset/{asset_id}`: latest job of an asset
/// (newest `created_at`). Server source of truth for the media UI overlay
/// "HD version in preparation" while the job is `queued|running`; 404 when
/// the asset was never stitched on the server.
#[utoipa::path(
    get,
    path = "/api/v1/stitch-jobs/by-asset/{asset_id}",
    tag = "stitch-jobs",
    operation_id = "stitch_jobs_get_by_asset",
    summary = "Get the latest stitch job of a media asset",
    description = "Most recently created stitch job of the asset (shows whether an HD version is being prepared).",
    params(("asset_id" = String, Path, format = "uuid", description = "Media asset UUID")),
    responses(
        (status = 200, body = StitchJobDto),
        (status = 404, description = "The asset was never stitched on the server", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn latest_for_asset(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(asset_id): Path<Uuid>,
) -> Result<Response> {
    let job = stitch_jobs::Entity::find()
        .filter(stitch_jobs::Column::OrgId.eq(org.org.id))
        .filter(stitch_jobs::Column::AssetId.eq(asset_id))
        .order_by_desc(stitch_jobs::Column::CreatedAt)
        .one(&ctx.db)
        .await
        .map_err(|_| Error::InternalServerError)?;
    match job {
        Some(j) => Ok(format::json(job_dto(&j)).into_response()),
        None => Err(Error::NotFound),
    }
}

// ─────────────────────────── Routes ───────────────────────────

/// OpenAPI paths of this controller (merged by `crate::openapi::spec`).
pub fn openapi() -> utoipa::openapi::OpenApi {
    #[derive(utoipa::OpenApi)]
    #[openapi(paths(create, latest_for_asset, detail, upload_frame))]
    struct Doc;
    <Doc as utoipa::OpenApi>::openapi()
}

pub fn routes() -> Routes {
    Routes::new()
        .prefix("/api/v1/stitch-jobs")
        .add("", post(create))
        // 2 segments after the prefix: unambiguous with GET /{id} (1).
        .add("/by-asset/{asset_id}", get(latest_for_asset))
        .add("/{id}", get(detail))
        .add(
            "/{id}/frames/{k}",
            // Limit dedicated to THIS route (frame ~1 MB, 8 MB cap + 1 MB
            // margin for the handler's clean JSON 413).
            post(upload_frame)
                .layer(DefaultBodyLimit::max(FRAME_MAX_BYTES + 1024 * 1024))
                .layer(axum::middleware::from_fn(
                    crate::services::compute_limits::upload_gate,
                )),
        )
}
