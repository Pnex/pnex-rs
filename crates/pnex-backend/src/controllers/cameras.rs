//! Cameras & recordings API (camera-video.md D76/D79/D80) — org-scoped
//! (`X-Org-Id`, cross-org = masking 404), writes gated by `can_write()`.
//!
//! - `GET    /api/v1/cameras` — cameras of the org (settings + live state)
//! - `GET    /api/v1/cameras/{device}/settings`
//! - `PATCH  /api/v1/cameras/{device}/settings` — validated, pushed live
//! - `GET    /api/v1/cameras/{device}/snapshot` — last received JPEG
//! - `GET    /api/v1/cameras/segments?device=&from=&to=` — D14 envelope
//! - `GET    /api/v1/cameras/segments/{id}/content` — MJPEG-AVI bytes
//! - `DELETE /api/v1/cameras/segments/{id}` — blob then row

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

fn field_errors(errs: Vec<(&'static str, String)>) -> Response {
    let body: serde_json::Map<String, serde_json::Value> = errs
        .into_iter()
        .map(|(k, v)| (k.to_string(), serde_json::Value::String(v)))
        .collect();
    (StatusCode::BAD_REQUEST, format::json(body)).into_response()
}

pub fn routes() -> Routes {
    Routes::new()
        .prefix("/api/v1/cameras")
        .add("", get(list))
        .add("/segments", get(list_segments))
        .add("/segments/{id}/content", get(segment_content))
        .add("/segments/{id}", get(segment_detail).delete(delete_segment))
        .add(
            "/{device}/settings",
            get(get_settings).patch(patch_settings),
        )
        .add("/{device}/snapshot", get(snapshot))
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

async fn list(State(ctx): State<AppContext>, org: OrgContext) -> Result<Response> {
    let rows = device_cameras::Entity::find()
        .filter(device_cameras::Column::OrgId.eq(org.org.id))
        .find_also_related(device_registries::Entity)
        .all(&ctx.db)
        .await
        .map_err(|_| Error::InternalServerError)?;
    let keys: Vec<(i64, i64, &str)> = rows
        .iter()
        .filter_map(|(_, dev)| dev.as_ref().map(|d| (d.id, d.org_id, d.device_id.as_str())))
        .collect();
    let mut live = camera::live_states(&keys).await;
    let mut views: Vec<CameraView> = rows
        .iter()
        .filter_map(|(cam, dev)| {
            dev.as_ref()
                .map(|d| view_of(cam, d, live.remove(&d.id).unwrap_or_default()))
        })
        .collect();
    views.sort_by(|a, b| a.device_id.cmp(&b.device_id));
    format::json(views)
}

async fn get_settings(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(device): Path<i64>,
) -> Result<Response> {
    let Some((cam, _)) = find_camera(&ctx.db, &org, device).await? else {
        return Err(Error::NotFound);
    };
    format::json(camera::settings_of(&cam))
}

/// Partial settings update — every field optional.
#[derive(Debug, Deserialize)]
pub struct SettingsPatch {
    framesize: Option<String>,
    quality: Option<u8>,
    fps: Option<u8>,
    capture_mode: Option<String>,
    vflip: Option<bool>,
    hmirror: Option<bool>,
}

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
            None => errs.push(("framesize", "invalid".to_string())),
        }
    }
    if let Some(m) = patch.capture_mode.as_deref() {
        match CaptureMode::from_wire(m) {
            Some(m) => next.capture_mode = m,
            None => errs.push(("capture_mode", "invalid".to_string())),
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

async fn snapshot(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(device): Path<i64>,
) -> Result<Response> {
    let Some((_, dev)) = find_camera(&ctx.db, &org, device).await? else {
        return Err(Error::NotFound);
    };
    // Local frame, else the last frame on the cluster bus (uplink on
    // another pod).
    let Some(frame) = camera::latest_anywhere(dev.org_id, dev.id, &dev.device_id).await else {
        return Err(Error::CustomError(
            StatusCode::NOT_FOUND,
            loco_rs::controller::ErrorDetail::new(
                "camera-no-frame",
                "No image received from this camera yet",
            ),
        ));
    };
    Ok((
        StatusCode::OK,
        [
            ("content-type", "image/jpeg".to_string()),
            ("cache-control", "no-store".to_string()),
        ],
        frame.jpeg.clone(),
    )
        .into_response())
}

// ───────────────────────────── Segments ─────────────────────────────

#[derive(Debug, Deserialize)]
pub struct SegmentQuery {
    device: Option<i64>,
    /// RFC 3339 lower bound on `started_at`.
    from: Option<String>,
    /// RFC 3339 upper bound on `started_at`.
    to: Option<String>,
    limit: Option<String>,
    offset: Option<String>,
}

fn parse_ts(raw: Option<&str>) -> Option<chrono::DateTime<chrono::FixedOffset>> {
    raw.and_then(|s| chrono::DateTime::parse_from_rfc3339(s.trim()).ok())
}

pub(crate) fn segment_dto(m: &video_segments::Model, device_id: &str) -> VideoSegment {
    VideoSegment {
        id: m.id.to_string(),
        device: m.device_registry_id,
        device_id: device_id.to_string(),
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

async fn list_segments(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Query(q): Query<SegmentQuery>,
) -> Result<Response> {
    let page = pagination::PageParams::from(q.limit.as_deref(), q.offset.as_deref());
    let mut filters = Vec::new();
    let mut query =
        video_segments::Entity::find().filter(video_segments::Column::OrgId.eq(org.org.id));
    if let Some(d) = q.device {
        query = query.filter(video_segments::Column::DeviceRegistryId.eq(d));
        filters.push(("device".to_string(), d.to_string()));
    }
    if let Some(from) = parse_ts(q.from.as_deref()) {
        query = query.filter(video_segments::Column::StartedAt.gte(from));
        filters.push(("from".to_string(), from.to_rfc3339()));
    }
    if let Some(to) = parse_ts(q.to.as_deref()) {
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

#[derive(Debug, Deserialize)]
pub struct ContentQuery {
    /// `1` → `Content-Disposition: attachment`.
    download: Option<String>,
}

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
    let device_id = dev.map(|d| d.device_id).unwrap_or_default();
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
