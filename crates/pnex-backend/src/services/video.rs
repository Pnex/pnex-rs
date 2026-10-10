//! Recorded video segments (camera-video.md D78/D79) — storage + rows +
//! retention pruner.
//!
//! Write order: **storage first, row second** (an orphan blob is purgeable,
//! a row without its blob is not — same rule as media D21); a DB failure
//! removes the freshly written blob best-effort. Delete order: blob, then
//! row (the store delete is idempotent).

use chrono::{DateTime, FixedOffset, TimeDelta, Utc};
use loco_rs::prelude::*;
use sea_orm::{ActiveValue::Set, ColumnTrait, EntityTrait, QueryFilter, QuerySelect};
use uuid::Uuid;

use crate::models::_entities::video_segments;
use crate::services::media::MediaSettings;

/// Pruner cadence (hourly — segments expire in days).
const PRUNE_INTERVAL_SECS: u64 = 3600;
/// Rows handled per prune batch.
const PRUNE_BATCH: u64 = 500;

/// What a segment records (D175): a device camera or an IP stream.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SegmentSource {
    /// `device_registries.id`.
    Device(i64),
    /// `media_streams.id`.
    Stream(Uuid),
}

/// Metadata of a segment to store.
#[derive(Debug, Clone)]
pub struct NewSegment {
    pub org_id: i64,
    pub source: SegmentSource,
    pub flow_id: Option<i64>,
    pub node_id: String,
    pub stream: String,
    pub started_at: DateTime<Utc>,
    pub ended_at: DateTime<Utc>,
    pub frame_count: i32,
    pub width: i32,
    pub height: i32,
    /// 0 = keep forever.
    pub retention_days: u32,
}

/// Logical storage key: `org_{org}/video/{device}/{yyyy}/{mm}/{dd}/{id}.avi`,
/// `org_{org}/video/stream_{uuid}/…` for an IP stream.
pub fn storage_key(
    org_id: i64,
    source: SegmentSource,
    started_at: DateTime<Utc>,
    id: Uuid,
) -> String {
    let dir = match source {
        SegmentSource::Device(device) => device.to_string(),
        SegmentSource::Stream(stream) => format!("stream_{stream}"),
    };
    format!(
        "org_{org_id}/video/{dir}/{}/{id}.avi",
        started_at.format("%Y/%m/%d")
    )
}

pub async fn write_segment(
    ctx: &AppContext,
    seg: NewSegment,
    bytes: axum::body::Bytes,
) -> Result<video_segments::Model> {
    let store = MediaSettings::from_config(&ctx.config)
        .store()
        .map_err(|e| {
            tracing::error!(error = %e, "video: media store unavailable");
            Error::InternalServerError
        })?;
    let id = Uuid::new_v4();
    let key = storage_key(seg.org_id, seg.source, seg.started_at, id);
    let size_bytes = bytes.len();
    store.put(&key, bytes).await.map_err(|e| {
        tracing::error!(error = %e, key, "video: segment write failed");
        Error::InternalServerError
    })?;
    let now: DateTime<FixedOffset> = Utc::now().into();
    let expires_at = (seg.retention_days > 0)
        .then(|| (seg.ended_at + TimeDelta::days(i64::from(seg.retention_days))).into());
    let row = video_segments::ActiveModel {
        id: Set(id),
        org_id: Set(seg.org_id),
        device_registry_id: Set(match seg.source {
            SegmentSource::Device(d) => Some(d),
            SegmentSource::Stream(_) => None,
        }),
        stream_id: Set(match seg.source {
            SegmentSource::Stream(s) => Some(s),
            SegmentSource::Device(_) => None,
        }),
        flow_id: Set(seg.flow_id),
        node_id: Set(seg.node_id),
        stream: Set(seg.stream),
        started_at: Set(seg.started_at.into()),
        ended_at: Set(seg.ended_at.into()),
        frame_count: Set(seg.frame_count),
        size_bytes: Set(size_bytes as i64),
        width: Set(seg.width),
        height: Set(seg.height),
        storage_key: Set(key.clone()),
        expires_at: Set(expires_at),
        created_at: Set(now),
        updated_at: Set(now),
    }
    .insert(&ctx.db)
    .await;
    match row {
        Ok(row) => Ok(row),
        Err(e) => {
            tracing::error!(error = %e, "video: segment row insert failed — removing blob");
            let _ = store.delete(&key).await;
            Err(Error::InternalServerError)
        }
    }
}

pub async fn delete_segment(ctx: &AppContext, seg: &video_segments::Model) -> Result<()> {
    let store = MediaSettings::from_config(&ctx.config)
        .store()
        .map_err(|_| Error::InternalServerError)?;
    store
        .delete(&seg.storage_key)
        .await
        .map_err(|_| Error::InternalServerError)?;
    video_segments::Entity::delete_by_id(seg.id)
        .exec(&ctx.db)
        .await
        .map_err(|_| Error::InternalServerError)?;
    Ok(())
}

/// Deletes expired segments (blob then row), in batches. Returns the count.
pub async fn prune_expired(ctx: &AppContext) -> Result<u64> {
    let now: DateTime<FixedOffset> = Utc::now().into();
    let mut total = 0;
    loop {
        let batch = video_segments::Entity::find()
            .filter(video_segments::Column::ExpiresAt.lt(now))
            .limit(PRUNE_BATCH)
            .all(&ctx.db)
            .await
            .map_err(|_| Error::InternalServerError)?;
        if batch.is_empty() {
            break;
        }
        let n = batch.len() as u64;
        for seg in &batch {
            delete_segment(ctx, seg).await?;
        }
        total += n;
        if n < PRUNE_BATCH {
            break;
        }
    }
    Ok(total)
}

/// Background pruner, started at boot — skipped in tests
/// (`ForegroundBlocking`), where `prune_expired` is called directly.
pub fn spawn_pruner(ctx: &AppContext) {
    use loco_rs::config::WorkerMode;
    if matches!(ctx.config.workers.mode, WorkerMode::ForegroundBlocking) {
        return;
    }
    let ctx = ctx.clone();
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(std::time::Duration::from_secs(PRUNE_INTERVAL_SECS));
        loop {
            tick.tick().await;
            // One pod prunes (D106): concurrent deletes of the same segment
            // would race on the storage.
            if !crate::services::singleton::my_turn(
                &ctx.db,
                "video-pruner",
                std::time::Duration::from_secs(PRUNE_INTERVAL_SECS),
            )
            .await
            {
                continue;
            }
            match prune_expired(&ctx).await {
                Ok(n) if n > 0 => tracing::info!(pruned = n, "video segment pruner"),
                Ok(_) => {}
                Err(e) => tracing::warn!(error = %e, "video segment pruner failed"),
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn key_layout() {
        let at = DateTime::parse_from_rfc3339("2026-09-29T10:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        let id = Uuid::nil();
        assert_eq!(
            storage_key(3, SegmentSource::Device(42), at, id),
            "org_3/video/42/2026/09/29/00000000-0000-0000-0000-000000000000.avi"
        );
        assert_eq!(
            storage_key(3, SegmentSource::Stream(id), at, id),
            format!("org_3/video/stream_{id}/2026/09/29/{id}.avi")
        );
    }
}
