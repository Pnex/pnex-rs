//! Captured audio segments (media-ingest.md D161, D162): bytes in the
//! MediaStore under a server-built key (R18), state in `media_segments`.

use std::sync::Arc;

use chrono::{DateTime, Utc};
use pnex_core::media_ingest::{ClockSource, SegmentState};
use sea_orm::{ActiveModelTrait, DatabaseConnection, DbErr, Set};
use uuid::Uuid;

use crate::models::_entities::{media_segments, media_streams};
use crate::services::media::MediaStore;

/// `org_{org}/media/{stream}/{yyyy}/{mm}/{dd}/{segment}.wav`.
pub fn storage_key(org_id: i64, stream_id: Uuid, started_at: DateTime<Utc>, id: Uuid) -> String {
    format!(
        "org_{org_id}/media/{stream_id}/{}/{id}.wav",
        started_at.format("%Y/%m/%d")
    )
}

/// A cut segment to store.
pub struct NewSegment {
    pub seq: i64,
    pub started_at: DateTime<Utc>,
    pub ended_at: DateTime<Utc>,
    pub clock_source: ClockSource,
    pub wav: Vec<u8>,
}

#[derive(Debug, thiserror::Error)]
pub enum SegmentError {
    #[error("media store: {0}")]
    Store(String),
    #[error(transparent)]
    Db(#[from] DbErr),
}

/// Stores the blob, then the row (`captured`). A failed insert removes the
/// blob: no orphan audio.
pub async fn write(
    db: &DatabaseConnection,
    store: &Arc<dyn MediaStore>,
    stream: &media_streams::Model,
    seg: NewSegment,
) -> Result<media_segments::Model, SegmentError> {
    let id = Uuid::new_v4();
    let key = storage_key(stream.org_id, stream.id, seg.started_at, id);
    let size = seg.wav.len() as i64;
    store
        .put(&key, seg.wav.into())
        .await
        .map_err(|e| SegmentError::Store(e.to_string()))?;
    let now: sea_orm::prelude::DateTimeWithTimeZone = Utc::now().into();
    let row = media_segments::ActiveModel {
        id: Set(id),
        org_id: Set(stream.org_id),
        stream_id: Set(stream.id),
        seq: Set(seg.seq),
        started_at: Set(seg.started_at.into()),
        ended_at: Set(seg.ended_at.into()),
        clock_source: Set(seg.clock_source.wire().into()),
        storage_key: Set(Some(key.clone())),
        size_bytes: Set(size),
        state: Set(SegmentState::Captured.wire().into()),
        asr_model_id: Set(None),
        asr_model_version: Set(None),
        asr_ms: Set(None),
        error: Set(None),
        created_at: Set(now),
        updated_at: Set(now),
    }
    .insert(db)
    .await;
    match row {
        Ok(row) => Ok(row),
        Err(e) => {
            let _ = store.delete(&key).await;
            Err(e.into())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn key_is_built_from_ids_only() {
        let t = DateTime::parse_from_rfc3339("2026-10-10T08:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        let s = Uuid::from_u128(1);
        let id = Uuid::from_u128(2);
        assert_eq!(
            storage_key(7, s, t, id),
            format!("org_7/media/{s}/2026/10/10/{id}.wav")
        );
    }
}
