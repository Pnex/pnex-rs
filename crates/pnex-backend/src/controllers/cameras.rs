//! Cameras & recordings API (camera-video.md D76/D79/D80) — org-scoped
//! (`X-Org-Id`, cross-org = masking 404), writes gated by `can_write()`.
//!
//! - `GET    /api/v1/cameras`: cameras of the org (settings + live state), paginated
//! - `PATCH  /api/v1/cameras/{device}/settings`: validated, pushed live
//! - `GET    /api/v1/cameras/segments?device=&stream_id=&from=&to=`: paginated
//!   (an unparsable `from`/`to` is a 400 `field-invalid`)
//! - `GET    /api/v1/cameras/segments/{id}`: one segment
//! - `GET    /api/v1/cameras/segments/{id}/content`: MJPEG-AVI bytes
//! - `DELETE /api/v1/cameras/segments/{id}`: blob then row
//!
//! The recording timeline lives in `camera_recordings.rs`.

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use loco_rs::controller::format;
use loco_rs::prelude::*;
use sea_orm::{
    ActiveModelTrait, ColumnTrait, EntityTrait, PaginatorTrait, QueryFilter, QueryOrder,
    QuerySelect, Set,
};
use serde::Deserialize;
use uuid::Uuid;

use crate::auth::OrgContext;
use crate::controllers::pagination;
use crate::models::_entities::{device_cameras, device_registries, video_segments};
use crate::services::camera;
use crate::services::media::MediaSettings;
use pnex_core::camera::{CameraView, CaptureMode, FrameSize, VideoSegment};

fn forbidden() -> Error {
    Error::CustomError(
        StatusCode::FORBIDDEN,
        loco_rs::controller::ErrorDetail::new(
            "camera-write-forbidden",
            "Changing camera settings or recordings requires the owner, admin or member role",
        ),
    )
}

/// Page parameters of the camera list.
#[derive(Debug, Deserialize, utoipa::IntoParams)]
#[into_params(parameter_in = Query)]
pub struct CameraListQuery {
    /// Page size (default 10, max 100).
    #[param(value_type = Option<i64>)]
    limit: Option<String>,
    #[param(value_type = Option<i64>)]
    offset: Option<String>,
}

fn field_errors(errs: Vec<(&'static str, String)>) -> Response {
    let fields = errs
        .into_iter()
        .map(|(k, v)| (k.to_string(), serde_json::Value::String(v)))
        .collect();
    super::fields_error(StatusCode::BAD_REQUEST, fields)
}

pub fn routes() -> Routes {
    Routes::new()
        .prefix("/api/v1/cameras")
        .add("", get(list))
        .add("/segments", get(list_segments))
        .add("/segments/{id}/content", get(segment_content))
        .add("/segments/{id}", get(segment_detail).delete(delete_segment))
        .add("/{device}/settings", patch(patch_settings))
}

/// OpenAPI paths of this controller (merged by `crate::openapi::spec`).
pub fn openapi() -> utoipa::openapi::OpenApi {
    #[derive(utoipa::OpenApi)]
    #[openapi(paths(
        list,
        list_segments,
        segment_content,
        segment_detail,
        delete_segment,
        patch_settings
    ))]
    struct Doc;
    <Doc as utoipa::OpenApi>::openapi()
}

/// Camera row + device of the org, else None (masking 404).
async fn find_camera(
    db: &DatabaseConnection,
    org: &OrgContext,
    device: i64,
) -> Result<Option<(device_cameras::Model, device_registries::Model)>> {
    let found = device_cameras::Entity::find_by_id(device)
        .filter(device_cameras::Column::OrgId.eq(org.org.id))
        .find_also_related(device_registries::Entity)
        .one(db)
        .await
        .map_err(|_| Error::InternalServerError)?;
    Ok(match found {
        Some((cam, Some(dev))) if dev.org_id == org.org.id => Some((cam, dev)),
        _ => None,
    })
}

/// `live` = cluster-wide state (`camera::live_states`): correct whichever
/// pod holds the uplink and the viewers.
fn view_of(
    cam: &device_cameras::Model,
    dev: &device_registries::Model,
    live: camera::LiveState,
) -> CameraView {
    CameraView {
        device: dev.id,
        device_id: dev.device_id.clone(),
        settings: camera::settings_of(cam),
        streaming: live.uplink,
        viewers: live.viewers,
        last_frame_ms: live.last_frame_ms,
    }
}

#[utoipa::path(
    get,
    path = "/api/v1/cameras",
    tag = "cameras",
    operation_id = "cameras_list",
    summary = "List the cameras of the organisation",
    description = "Paginated camera devices (`limit` default 10, max 100; `offset`) with their settings and live state (streaming uplink, viewer count, last frame time), sorted by device id.",
    params(CameraListQuery),
    responses(
        (status = 200, body = pnex_core::Paginated<pnex_core::camera::CameraView>)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn list(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Query(q): Query<CameraListQuery>,
) -> Result<Response> {
    let page = pagination::PageParams::from(q.limit.as_deref(), q.offset.as_deref());
    let select = device_cameras::Entity::find()
        .filter(device_cameras::Column::OrgId.eq(org.org.id))
        .inner_join(device_registries::Entity)
        .filter(device_registries::Column::OrgId.eq(org.org.id))
        .order_by_asc(device_registries::Column::DeviceId);
    let count = select
        .clone()
        .count(&ctx.db)
        .await
        .map_err(|_| Error::InternalServerError)? as i64;
    let rows = select
        .offset(page.offset as u64)
        .limit(page.limit as u64)
        .select_also(device_registries::Entity)
        .all(&ctx.db)
        .await
        .map_err(|_| Error::InternalServerError)?;
    let keys: Vec<(i64, i64, &str)> = rows
        .iter()
        .filter_map(|(_, dev)| dev.as_ref().map(|d| (d.id, d.org_id, d.device_id.as_str())))
        .collect();
    let mut live = camera::live_states(&keys).await;
    let views: Vec<CameraView> = rows
        .iter()
        .filter_map(|(cam, dev)| {
            dev.as_ref()
                .map(|d| view_of(cam, d, live.remove(&d.id).unwrap_or_default()))
        })
        .collect();
    format::json(pagination::envelope(
        "/api/v1/cameras",
        &[],
        page,
        count,
        views,
    ))
}

/// Partial settings update — every field optional.
#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct SettingsPatch {
    /// Resolution (`qvga`, `cif`, `vga`, `svga`, `xga`, `hd`, `sxga`, `uxga`).
    /// Read as a string so a bad value is a `field-invalid` token.
    #[schema(value_type = Option<pnex_core::camera::FrameSize>)]
    framesize: Option<String>,
    /// JPEG quality (lower = better).
    quality: Option<u8>,
    fps: Option<u8>,
    /// `on_demand` or `continuous`.
    #[schema(value_type = Option<pnex_core::camera::CaptureMode>)]
    capture_mode: Option<String>,
    vflip: Option<bool>,
    hmirror: Option<bool>,
}

#[utoipa::path(
    patch,
    path = "/api/v1/cameras/{device}/settings",
    tag = "cameras",
    operation_id = "cameras_settings_update",
    summary = "Change a camera's settings",
    description = "Partial update (absent fields unchanged) of resolution, quality, fps, capture mode and flips. The new settings are validated, saved and pushed live to the device. Writer role required.",
    params(("device" = i64, Path, description = "Device registry id of the camera")),
    request_body = SettingsPatch,
    responses(
        (status = 200, body = pnex_core::camera::CameraView),
        (status = 400, description = "`field-invalid`: `framesize`/`capture_mode` `invalid`, `quality`/`fps` `range:<min>..<max>`", body = crate::openapi::ErrorDetail),
        (status = 403, description = "Viewer role (`camera-write-forbidden`)", body = crate::openapi::ErrorDetail),
        (status = 404, description = "Unknown camera or camera of another organisation", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn patch_settings(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(device): Path<i64>,
    axum::Json(patch): axum::Json<SettingsPatch>,
) -> Result<Response> {
    if !org.can_write() {
        return Err(forbidden());
    }
    let Some((cam, dev)) = find_camera(&ctx.db, &org, device).await? else {
        return Err(Error::NotFound);
    };
    let mut next = camera::settings_of(&cam);
    let mut errs = Vec::new();
    if let Some(f) = patch.framesize.as_deref() {
        match FrameSize::from_wire(f) {
            Some(f) => next.framesize = f,
            None => errs.push(("framesize", pnex_core::err_codes::FIELD_INVALID.to_string())),
        }
    }
    if let Some(m) = patch.capture_mode.as_deref() {
        match CaptureMode::from_wire(m) {
            Some(m) => next.capture_mode = m,
            None => errs.push((
                "capture_mode",
                pnex_core::err_codes::FIELD_INVALID.to_string(),
            )),
        }
    }
    if let Some(q) = patch.quality {
        next.quality = q;
    }
    if let Some(f) = patch.fps {
        next.fps = f;
    }
    if let Some(v) = patch.vflip {
        next.vflip = v;
    }
    if let Some(h) = patch.hmirror {
        next.hmirror = h;
    }
    if let Err(more) = next.check() {
        errs.extend(more);
    }
    if !errs.is_empty() {
        return Ok(field_errors(errs));
    }
    let mut upd: device_cameras::ActiveModel = cam.into();
    upd.framesize = Set(next.framesize.wire().to_string());
    upd.quality = Set(i16::from(next.quality));
    upd.fps = Set(i16::from(next.fps));
    upd.capture_mode = Set(next.capture_mode.wire().to_string());
    upd.vflip = Set(next.vflip);
    upd.hmirror = Set(next.hmirror);
    upd.updated_at = Set(chrono::Utc::now().into());
    let saved = upd
        .update(&ctx.db)
        .await
        .map_err(|_| Error::InternalServerError)?;
    camera::push_config(&ctx.db, dev.id).await;
    let live = camera::live_states(&[(dev.id, dev.org_id, dev.device_id.as_str())])
        .await
        .remove(&dev.id)
        .unwrap_or_default();
    format::json(view_of(&saved, &dev, live))
}

// ───────────────────────────── Segments ─────────────────────────────

#[derive(Debug, Deserialize, utoipa::IntoParams)]
#[into_params(parameter_in = Query)]
pub struct SegmentQuery {
    /// Device registry id of the camera.
    device: Option<i64>,
    /// Recordings of one IP stream (`media_streams.id`, D175).
    #[param(value_type = Option<String>, format = "uuid")]
    stream_id: Option<Uuid>,
    /// RFC 3339 lower bound on `started_at`.
    #[param(format = DateTime)]
    from: Option<String>,
    /// RFC 3339 upper bound on `started_at`.
    #[param(format = DateTime)]
    to: Option<String>,
    /// Page size (default 10, max 100).
    #[param(value_type = Option<i64>)]
    limit: Option<String>,
    #[param(value_type = Option<i64>)]
    offset: Option<String>,
}

/// Optional RFC 3339 bound: absent/blank → `Ok(None)`, unparsable → `Err`
/// (O80: a bad bound is a 400, never silently ignored).
fn parse_ts(
    raw: Option<&str>,
) -> std::result::Result<Option<chrono::DateTime<chrono::FixedOffset>>, ()> {
    match raw.map(str::trim).filter(|s| !s.is_empty()) {
        None => Ok(None),
        Some(s) => chrono::DateTime::parse_from_rfc3339(s)
            .map(Some)
            .map_err(|_| ()),
    }
}

pub(crate) fn segment_dto(m: &video_segments::Model, device_id: &str) -> VideoSegment {
    VideoSegment {
        id: m.id.to_string(),
        device: m.device_registry_id.unwrap_or_default(),
        device_id: device_id.to_string(),
        stream_id: m.stream_id.map(|s| s.to_string()),
        stream: m.stream.clone(),
        flow_id: m.flow_id,
        node_id: m.node_id.clone(),
        started_at: m.started_at.to_rfc3339(),
        ended_at: m.ended_at.to_rfc3339(),
        frame_count: m.frame_count,
        size_bytes: m.size_bytes,
        width: m.width,
        height: m.height,
        expires_at: m.expires_at.map(|e| e.to_rfc3339()),
    }
}

#[utoipa::path(
    get,
    path = "/api/v1/cameras/segments",
    tag = "cameras",
    operation_id = "cameras_segments_list",
    summary = "List recorded video segments",
    description = "Paginated recorded segments of the organisation, newest first, optionally filtered by camera, IP stream and start time window.",
    params(SegmentQuery),
    responses(
        (status = 200, body = pnex_core::Paginated<pnex_core::camera::VideoSegment>),
        (status = 400, description = "Unparsable `from`/`to` (`field-invalid`, token `invalid`)", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn list_segments(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Query(q): Query<SegmentQuery>,
) -> Result<Response> {
    let page = pagination::PageParams::from(q.limit.as_deref(), q.offset.as_deref());
    let mut bad = Vec::new();
    let from = parse_ts(q.from.as_deref()).unwrap_or_else(|()| {
        bad.push(("from", pnex_core::err_codes::FIELD_INVALID.to_string()));
        None
    });
    let to = parse_ts(q.to.as_deref()).unwrap_or_else(|()| {
        bad.push(("to", pnex_core::err_codes::FIELD_INVALID.to_string()));
        None
    });
    if !bad.is_empty() {
        return Ok(field_errors(bad));
    }
    let mut filters = Vec::new();
    let mut query =
        video_segments::Entity::find().filter(video_segments::Column::OrgId.eq(org.org.id));
    if let Some(d) = q.device {
        query = query.filter(video_segments::Column::DeviceRegistryId.eq(d));
        filters.push(("device".to_string(), d.to_string()));
    }
    if let Some(sid) = q.stream_id {
        query = query.filter(video_segments::Column::StreamId.eq(sid));
        filters.push(("stream_id".to_string(), sid.to_string()));
    }
    if let Some(from) = from {
        query = query.filter(video_segments::Column::StartedAt.gte(from));
        filters.push(("from".to_string(), from.to_rfc3339()));
    }
    if let Some(to) = to {
        query = query.filter(video_segments::Column::StartedAt.lt(to));
        filters.push(("to".to_string(), to.to_rfc3339()));
    }
    let count = query
        .clone()
        .count(&ctx.db)
        .await
        .map_err(|_| Error::InternalServerError)? as i64;
    let rows = query
        .order_by_desc(video_segments::Column::StartedAt)
        .offset(page.offset as u64)
        .limit(page.limit as u64)
        .find_also_related(device_registries::Entity)
        .all(&ctx.db)
        .await
        .map_err(|_| Error::InternalServerError)?;
    let results: Vec<VideoSegment> = rows
        .iter()
        .map(|(s, d)| segment_dto(s, d.as_ref().map(|d| d.device_id.as_str()).unwrap_or("")))
        .collect();
    format::json(pagination::envelope(
        "/api/v1/cameras/segments",
        &filters,
        page,
        count,
        results,
    ))
}

async fn find_segment(
    db: &DatabaseConnection,
    org: &OrgContext,
    id: Uuid,
) -> Result<Option<(video_segments::Model, Option<device_registries::Model>)>> {
    video_segments::Entity::find_by_id(id)
        .filter(video_segments::Column::OrgId.eq(org.org.id))
        .find_also_related(device_registries::Entity)
        .one(db)
        .await
        .map_err(|_| Error::InternalServerError)
}

#[utoipa::path(
    get,
    path = "/api/v1/cameras/segments/{id}",
    tag = "cameras",
    operation_id = "cameras_segments_get",
    summary = "Get a recorded video segment",
    description = "Metadata of one segment (time span, frames, size, resolution, expiry).",
    params(("id" = String, Path, format = "uuid", description = "Video segment UUID")),
    responses(
        (status = 200, body = pnex_core::camera::VideoSegment),
        (status = 404, description = "Unknown segment or segment of another organisation", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn segment_detail(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(id): Path<Uuid>,
) -> Result<Response> {
    let Some((seg, dev)) = find_segment(&ctx.db, &org, id).await? else {
        return Err(Error::NotFound);
    };
    format::json(segment_dto(
        &seg,
        dev.as_ref().map(|d| d.device_id.as_str()).unwrap_or(""),
    ))
}

#[derive(Debug, Deserialize, utoipa::IntoParams)]
#[into_params(parameter_in = Query)]
pub struct ContentQuery {
    /// `1` → `Content-Disposition: attachment`.
    download: Option<String>,
}

#[utoipa::path(
    get,
    path = "/api/v1/cameras/segments/{id}/content",
    tag = "cameras",
    operation_id = "cameras_segments_content",
    summary = "Download a recorded video segment",
    description = "MJPEG-AVI bytes of the segment; `download=1` sends it as an attachment.",
    params(("id" = String, Path, format = "uuid", description = "Video segment UUID"), ContentQuery),
    responses(
        (status = 200, content_type = "video/x-msvideo", body = Vec<u8>),
        (status = 404, description = "Unknown segment or stored file missing", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn segment_content(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(id): Path<Uuid>,
    Query(q): Query<ContentQuery>,
) -> Result<Response> {
    let Some((seg, dev)) = find_segment(&ctx.db, &org, id).await? else {
        return Err(Error::NotFound);
    };
    let store = MediaSettings::from_config(&ctx.config)
        .store()
        .map_err(|_| Error::InternalServerError)?;
    let bytes = store
        .get(&seg.storage_key)
        .await
        .map_err(|_| Error::NotFound)?;
    // A stream recording is named after its stream id (no device).
    let device_id = dev
        .map(|d| d.device_id)
        .or_else(|| seg.stream_id.map(|s| s.to_string()))
        .unwrap_or_default();
    let filename = pnex_firmware_builder::sanitize_segment(&format!(
        "{device_id}_{}.avi",
        seg.started_at.format("%Y%m%dT%H%M%S")
    ));
    let disposition = if q.download.as_deref() == Some("1") {
        "attachment"
    } else {
        "inline"
    };
    Ok((
        StatusCode::OK,
        [
            ("content-type", "video/x-msvideo".to_string()),
            (
                "content-disposition",
                format!("{disposition}; filename=\"{filename}\""),
            ),
        ],
        bytes,
    )
        .into_response())
}

#[utoipa::path(
    delete,
    path = "/api/v1/cameras/segments/{id}",
    tag = "cameras",
    operation_id = "cameras_segments_delete",
    summary = "Delete a recorded video segment",
    description = "Permanently deletes the stored file then the segment row. Writer role required.",
    params(("id" = String, Path, format = "uuid", description = "Video segment UUID")),
    responses(
        (status = 204, description = "Deleted"),
        (status = 403, description = "Viewer role (`camera-write-forbidden`)", body = crate::openapi::ErrorDetail),
        (status = 404, description = "Unknown segment", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn delete_segment(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(id): Path<Uuid>,
) -> Result<Response> {
    if !org.can_write() {
        return Err(forbidden());
    }
    let Some((seg, _)) = find_segment(&ctx.db, &org, id).await? else {
        return Err(Error::NotFound);
    };
    crate::services::video::delete_segment(&ctx, &seg).await?;
    Ok(StatusCode::NO_CONTENT.into_response())
}
