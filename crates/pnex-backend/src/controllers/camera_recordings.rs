//! Continuous recording of one camera (camera-video.md D79/D80) — the
//! segments stay small on disk (crash resilience, per-minute retention),
//! the API exposes them as one timeline:
//!
//! - `GET    /api/v1/cameras/{device}/recordings?from=&to=` — ascending
//!   segments of the window (≤ 26 h, ≤ 5000 rows), overlaps included
//! - `GET    /api/v1/cameras/{device}/recordings/export?from=&to=` — one
//!   MJPEG-AVI concatenating the chained segments (source bytes capped)
//! - `DELETE /api/v1/cameras/{device}/recordings?from=&to=` — every segment
//!   starting in the window (owner/admin/member)
//! - `GET    /api/v1/cameras/{device}/recordings/layers?from=&to=` —
//!   annotation layers (detection nodes, D105) stored in OpenObserve
//! - `GET    /api/v1/cameras/{device}/recordings/annotations?from=&to=&layers=a,b`
//!   — boxes of the selected layers, drawn over the playback
//!
//! Both answer empty when O2 is off: the overlay is optional.
//!
//! Org-scoped like the rest of `/api/v1/cameras` (cross-org = 404).

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use chrono::{DateTime, FixedOffset};
use loco_rs::controller::format;
use loco_rs::prelude::*;
use sea_orm::{ColumnTrait, EntityTrait, QueryFilter, QueryOrder, QuerySelect};
use serde::Deserialize;

use crate::auth::OrgContext;
use crate::models::_entities::{device_registries, video_segments};
use crate::services::media::MediaSettings;
use pnex_core::avi::{read_avi, write_avi};
use pnex_core::camera::{
    playback_chain, RecordingTimeline, EXPORT_MAX_BYTES, TIMELINE_MAX_SEGMENTS,
    TIMELINE_MAX_SPAN_SECS,
};
use pnex_core::err_codes;

use super::cameras::segment_dto;

/// Widest window of a bulk delete (the UI deletes one day at a time).
const DELETE_MAX_SPAN_SECS: i64 = 31 * 86_400;

pub fn routes() -> Routes {
    Routes::new()
        .prefix("/api/v1/cameras")
        .add("/{device}/recordings", get(timeline).delete(delete_range))
        .add("/{device}/recordings/export", get(export))
        .add("/{device}/recordings/layers", get(layers))
        .add("/{device}/recordings/annotations", get(annotations))
}

fn coded(status: StatusCode, code: &str, message: &str) -> Error {
    Error::CustomError(status, loco_rs::controller::ErrorDetail::new(code, message))
}

fn range_invalid() -> Error {
    coded(
        StatusCode::BAD_REQUEST,
        err_codes::CAMERA_RANGE_INVALID,
        "Expected RFC 3339 `from` < `to` within the allowed window",
    )
}

#[derive(Debug, Deserialize)]
pub struct RangeQuery {
    from: Option<String>,
    to: Option<String>,
    /// Comma-separated layer ids (annotations only).
    #[serde(default)]
    layers: Option<String>,
}

/// Parses the mandatory `[from, to)` window, at most `max_secs` wide.
fn window(q: &RangeQuery, max_secs: i64) -> Result<(DateTime<FixedOffset>, DateTime<FixedOffset>)> {
    let parse = |raw: Option<&str>| raw.and_then(|s| DateTime::parse_from_rfc3339(s.trim()).ok());
    let (Some(from), Some(to)) = (parse(q.from.as_deref()), parse(q.to.as_deref())) else {
        return Err(range_invalid());
    };
    let span = (to - from).num_seconds();
    if span <= 0 || span > max_secs {
        return Err(range_invalid());
    }
    Ok((from, to))
}

/// Device of the org, else masking 404 (a device whose camera capability
/// was dropped keeps its recordings reachable).
async fn find_device(
    db: &DatabaseConnection,
    org: &OrgContext,
    device: i64,
) -> Result<device_registries::Model> {
    device_registries::Entity::find_by_id(device)
        .filter(device_registries::Column::OrgId.eq(org.org.id))
        .one(db)
        .await
        .map_err(|_| Error::InternalServerError)?
        .ok_or(Error::NotFound)
}

/// Segments of one device starting in `[from, to)`, ascending.
fn segments_query(
    org: &OrgContext,
    device: i64,
    from: DateTime<FixedOffset>,
    to: DateTime<FixedOffset>,
) -> sea_orm::Select<video_segments::Entity> {
    video_segments::Entity::find()
        .filter(video_segments::Column::OrgId.eq(org.org.id))
        .filter(video_segments::Column::DeviceRegistryId.eq(device))
        .filter(video_segments::Column::StartedAt.gte(from))
        .filter(video_segments::Column::StartedAt.lt(to))
        .order_by_asc(video_segments::Column::StartedAt)
}

fn span_ms(seg: &video_segments::Model) -> (i64, i64) {
    (
        seg.started_at.timestamp_millis(),
        seg.ended_at.timestamp_millis(),
    )
}

async fn timeline(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(device): Path<i64>,
    Query(q): Query<RangeQuery>,
) -> Result<Response> {
    let (from, to) = window(&q, TIMELINE_MAX_SPAN_SECS)?;
    let dev = find_device(&ctx.db, &org, device).await?;
    let mut rows = segments_query(&org, device, from, to)
        .limit(TIMELINE_MAX_SEGMENTS + 1)
        .all(&ctx.db)
        .await
        .map_err(|_| Error::InternalServerError)?;
    let truncated = rows.len() as u64 > TIMELINE_MAX_SEGMENTS;
    rows.truncate(TIMELINE_MAX_SEGMENTS as usize);
    let segments: Vec<_> = rows
        .iter()
        .map(|s| segment_dto(s, &dev.device_id))
        .collect();
    let total_bytes = rows.iter().map(|s| s.size_bytes).sum();
    format::json(RecordingTimeline {
        segments,
        total_bytes,
        truncated,
    })
}

async fn export(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(device): Path<i64>,
    Query(q): Query<RangeQuery>,
) -> Result<Response> {
    let (from, to) = window(&q, TIMELINE_MAX_SPAN_SECS)?;
    let dev = find_device(&ctx.db, &org, device).await?;
    let rows = segments_query(&org, device, from, to)
        .limit(TIMELINE_MAX_SEGMENTS)
        .all(&ctx.db)
        .await
        .map_err(|_| Error::InternalServerError)?;
    let spans: Vec<(i64, i64)> = rows.iter().map(span_ms).collect();
    let chain: Vec<&video_segments::Model> = playback_chain(&spans)
        .into_iter()
        .map(|i| &rows[i])
        .collect();
    if chain.is_empty() {
        return Err(coded(
            StatusCode::NOT_FOUND,
            err_codes::CAMERA_EXPORT_EMPTY,
            "No recording in this range",
        ));
    }
    let source_bytes: i64 = chain.iter().map(|s| s.size_bytes).sum();
    if source_bytes > EXPORT_MAX_BYTES {
        return Err(coded(
            StatusCode::PAYLOAD_TOO_LARGE,
            err_codes::CAMERA_EXPORT_TOO_LARGE,
            "This range is too large for a single file",
        ));
    }

    let store = MediaSettings::from_config(&ctx.config)
        .store()
        .map_err(|_| Error::InternalServerError)?;
    // Blobs first (kept alive), frames borrowed from them afterwards.
    let mut blobs = Vec::with_capacity(chain.len());
    for seg in &chain {
        match store.get(&seg.storage_key).await {
            Ok(bytes) => blobs.push(bytes),
            // A blob pruned between the query and the read: skip it.
            Err(e) => {
                tracing::warn!(error = %e, key = seg.storage_key, "export: segment unreadable")
            }
        }
    }
    let mut frames: Vec<&[u8]> = Vec::new();
    let (mut width, mut height) = (0u16, 0u16);
    let mut playing_secs = 0.0f64;
    for (blob, seg) in blobs.iter().zip(&chain) {
        let Ok(index) = read_avi(blob) else {
            continue;
        };
        if index.frames.is_empty() {
            continue;
        }
        // The file keeps the first resolution; a segment recorded at a
        // different size would not decode in the same stream.
        if width == 0 {
            width = index.width as u16;
            height = index.height as u16;
        } else if (index.width as u16, index.height as u16) != (width, height) {
            continue;
        }
        let (start, end) = span_ms(seg);
        playing_secs += ((end - start).max(0) as f64) / 1000.0;
        frames.extend(index.frames.iter().filter_map(|&(s, l)| blob.get(s..s + l)));
    }
    if frames.is_empty() {
        return Err(coded(
            StatusCode::NOT_FOUND,
            err_codes::CAMERA_EXPORT_EMPTY,
            "No recording in this range",
        ));
    }
    // Average real-time rate over the recorded spans (gaps are cut).
    let fps = if playing_secs > 0.0 {
        frames.len() as f64 / playing_secs
    } else {
        1.0
    };
    let avi = write_avi(width, height, fps, &frames);
    let filename = pnex_firmware_builder::sanitize_segment(&format!(
        "{}_{}_{}.avi",
        dev.device_id,
        chain[0].started_at.format("%Y%m%dT%H%M%S"),
        chain[chain.len() - 1].ended_at.format("%H%M%S"),
    ));
    Ok((
        StatusCode::OK,
        [
            ("content-type", "video/x-msvideo".to_string()),
            (
                "content-disposition",
                format!("attachment; filename=\"{filename}\""),
            ),
        ],
        avi,
    )
        .into_response())
}

async fn layers(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(device): Path<i64>,
    Query(q): Query<RangeQuery>,
) -> Result<Response> {
    let (from, to) = window(&q, TIMELINE_MAX_SPAN_SECS)?;
    let dev = find_device(&ctx.db, &org, device).await?;
    let layers = crate::services::video_annotations::layers(
        &ctx,
        org.org.id,
        &dev.device_id,
        from.timestamp_millis(),
        to.timestamp_millis(),
    )
    .await
    .unwrap_or_else(|e| {
        tracing::warn!(error = %e, "annotation layers: openobserve read failed");
        Vec::new()
    });
    format::json(layers)
}

async fn annotations(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(device): Path<i64>,
    Query(q): Query<RangeQuery>,
) -> Result<Response> {
    let (from, to) = window(&q, TIMELINE_MAX_SPAN_SECS)?;
    let dev = find_device(&ctx.db, &org, device).await?;
    let layers: Vec<String> = q
        .layers
        .as_deref()
        .unwrap_or_default()
        .split(',')
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .map(str::to_string)
        .collect();
    let anns = crate::services::video_annotations::search(
        &ctx,
        org.org.id,
        &dev.device_id,
        &layers,
        from.timestamp_millis(),
        to.timestamp_millis(),
    )
    .await
    .unwrap_or_else(|e| {
        tracing::warn!(error = %e, "annotations: openobserve read failed");
        Vec::new()
    });
    format::json(anns)
}

async fn delete_range(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(device): Path<i64>,
    Query(q): Query<RangeQuery>,
) -> Result<Response> {
    if !org.can_write() {
        return Err(coded(
            StatusCode::FORBIDDEN,
            err_codes::CAMERA_WRITE_FORBIDDEN,
            "Changing camera settings or recordings requires the owner, admin or member role",
        ));
    }
    let (from, to) = window(&q, DELETE_MAX_SPAN_SECS)?;
    find_device(&ctx.db, &org, device).await?;
    let mut deleted = 0u64;
    loop {
        let batch = segments_query(&org, device, from, to)
            .limit(500)
            .all(&ctx.db)
            .await
            .map_err(|_| Error::InternalServerError)?;
        if batch.is_empty() {
            break;
        }
        for seg in &batch {
            crate::services::video::delete_segment(&ctx, seg).await?;
            deleted += 1;
        }
    }
    format::json(serde_json::json!({ "deleted": deleted }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn q(from: &str, to: &str) -> RangeQuery {
        RangeQuery {
            from: Some(from.into()),
            to: Some(to.into()),
            layers: None,
        }
    }

    #[test]
    fn window_bounds() {
        assert!(window(
            &q("2026-09-30T00:00:00Z", "2026-10-01T00:00:00Z"),
            TIMELINE_MAX_SPAN_SECS
        )
        .is_ok());
        // Reversed, empty, too wide, missing.
        assert!(window(
            &q("2026-10-01T00:00:00Z", "2026-09-30T00:00:00Z"),
            TIMELINE_MAX_SPAN_SECS
        )
        .is_err());
        assert!(window(
            &q("2026-09-30T00:00:00Z", "2026-09-30T00:00:00Z"),
            TIMELINE_MAX_SPAN_SECS
        )
        .is_err());
        assert!(window(
            &q("2026-09-01T00:00:00Z", "2026-09-30T00:00:00Z"),
            TIMELINE_MAX_SPAN_SECS
        )
        .is_err());
        assert!(window(
            &RangeQuery {
                from: None,
                to: None,
                layers: None
            },
            TIMELINE_MAX_SPAN_SECS
        )
        .is_err());
    }
}
