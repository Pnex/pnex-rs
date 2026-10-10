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

/// Queue priority of a transcription (D166): on a dedicated `asr` queue,
/// live segments go before replays; the shared untagged queue keeps
/// priority 0 so transcriptions never overtake the other jobs.
pub fn priority(live: bool) -> Option<i32> {
    (live && !super::asr_queue_tags().is_empty()).then_some(1)
}

/// Marks the segment `queued` and enqueues its transcription (D166). A
/// failed enqueue leaves it `captured` (visible, replayable). `live` = a
/// segment just captured, `false` = a replay.
pub async fn enqueue(ctx: &loco_rs::app::AppContext, seg: &media_segments::Model, live: bool) {
    use crate::workers::transcribe_segment::{TranscribeSegmentArgs, TranscribeSegmentWorker};
    use loco_rs::bgworker::BackgroundWorker;
    use sea_orm::sea_query::Expr;
    use sea_orm::{ColumnTrait, EntityTrait, QueryFilter};
    let queued = media_segments::Entity::update_many()
        .col_expr(
            media_segments::Column::State,
            Expr::value(SegmentState::Queued.wire()),
        )
        .filter(media_segments::Column::Id.eq(seg.id))
        .exec(&ctx.db)
        .await;
    if queued.is_err() {
        return;
    }
    let args = TranscribeSegmentArgs {
        segment_id: seg.id,
        org_id: seg.org_id,
    };
    if let Err(e) =
        TranscribeSegmentWorker::perform_later_with_priority(ctx, args, priority(live)).await
    {
        tracing::warn!(segment = %seg.id, error = %e, "transcription not enqueued");
        let _ = media_segments::Entity::update_many()
            .col_expr(
                media_segments::Column::State,
                Expr::value(SegmentState::Captured.wire()),
            )
            .filter(media_segments::Column::Id.eq(seg.id))
            .exec(&ctx.db)
            .await;
    }
}

/// A failed or never-queued segment keeps its audio this long for an
/// explicit replay (D161: `asr_retry_window`), then the audio goes.
pub const RETRY_WINDOW_SECS: i64 = 15 * 60;
/// Period of the audio pruner.
const PRUNE_EVERY: std::time::Duration = std::time::Duration::from_secs(15 * 60);
/// Rows handled per pruner query.
const PRUNE_BATCH: u64 = 500;

/// Should the audio of `seg` go now, per its stream's retention (D161)?
pub fn audio_expired(
    seg: &media_segments::Model,
    retention: pnex_core::media_ingest::AudioRetention,
    now: DateTime<Utc>,
) -> bool {
    use pnex_core::media_ingest::AudioRetention;
    if seg.storage_key.is_none() {
        return false;
    }
    let state = SegmentState::from_wire(&seg.state);
    match retention {
        AudioRetention::Keep => false,
        AudioRetention::Days(n) => seg.ended_at < now - chrono::Duration::days(i64::from(n)),
        // Audio only waits for its transcription, or for a replay window.
        AudioRetention::None => match state {
            Some(SegmentState::Queued | SegmentState::Transcribing) => false,
            _ => seg.updated_at < now - chrono::Duration::seconds(RETRY_WINDOW_SECS),
        },
    }
}

/// Deletes the expired audio of every stream (blob, then key cleared).
/// Returns how many blobs went.
pub async fn prune_audio(
    db: &DatabaseConnection,
    store: &Arc<dyn MediaStore>,
) -> Result<u64, SegmentError> {
    use pnex_core::media_ingest::AudioRetention;
    use sea_orm::sea_query::Expr;
    use sea_orm::{ColumnTrait, EntityTrait, QueryFilter, QueryOrder, QuerySelect};
    let now = Utc::now();
    let streams: std::collections::HashMap<Uuid, AudioRetention> = media_streams::Entity::find()
        .all(db)
        .await?
        .into_iter()
        .map(|s| {
            (
                s.id,
                AudioRetention::from_wire(&s.audio_retention).unwrap_or_default(),
            )
        })
        .collect();
    let mut pruned = 0u64;
    let mut after: Option<Uuid> = None;
    loop {
        let mut q = media_segments::Entity::find()
            .filter(media_segments::Column::StorageKey.is_not_null())
            .order_by_asc(media_segments::Column::Id)
            .limit(PRUNE_BATCH);
        if let Some(id) = after {
            q = q.filter(media_segments::Column::Id.gt(id));
        }
        let rows = q.all(db).await?;
        let Some(last) = rows.last() else {
            break;
        };
        after = Some(last.id);
        for seg in rows.iter().filter(|s| {
            audio_expired(
                s,
                streams.get(&s.stream_id).copied().unwrap_or_default(),
                now,
            )
        }) {
            let Some(key) = seg.storage_key.as_deref() else {
                continue;
            };
            if let Err(e) = store.delete(key).await {
                tracing::warn!(segment = %seg.id, error = %e, "segment audio not pruned");
                continue;
            }
            media_segments::Entity::update_many()
                .col_expr(
                    media_segments::Column::StorageKey,
                    Expr::value(Option::<String>::None),
                )
                .filter(media_segments::Column::Id.eq(seg.id))
                .exec(db)
                .await?;
            pruned += 1;
        }
    }
    Ok(pruned)
}

/// Background audio pruner, one pod at a time (D106); skipped in tests
/// (`ForegroundBlocking`), where [`prune_audio`] is called directly.
pub fn spawn_pruner(ctx: &loco_rs::app::AppContext) {
    use loco_rs::config::WorkerMode;
    if matches!(ctx.config.workers.mode, WorkerMode::ForegroundBlocking) {
        return;
    }
    let ctx = ctx.clone();
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(PRUNE_EVERY);
        loop {
            tick.tick().await;
            if !crate::services::singleton::my_turn(&ctx.db, "media-audio-pruner", PRUNE_EVERY)
                .await
            {
                continue;
            }
            let Ok(store) = crate::services::media::MediaSettings::from_config(&ctx.config).store()
            else {
                continue;
            };
            match prune_audio(&ctx.db, &store).await {
                Ok(n) if n > 0 => tracing::info!(pruned = n, "media audio pruner"),
                Ok(_) => {}
                Err(e) => tracing::warn!(error = %e, "media audio pruner failed"),
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn seg(state: &str, ended_secs_ago: i64, updated_secs_ago: i64) -> media_segments::Model {
        let now = Utc::now();
        media_segments::Model {
            id: Uuid::nil(),
            org_id: 1,
            stream_id: Uuid::nil(),
            seq: 0,
            started_at: (now - chrono::Duration::seconds(ended_secs_ago + 30)).into(),
            ended_at: (now - chrono::Duration::seconds(ended_secs_ago)).into(),
            clock_source: "host".into(),
            storage_key: Some("k".into()),
            size_bytes: 1,
            state: state.into(),
            asr_model_id: None,
            asr_model_version: None,
            asr_ms: None,
            error: None,
            created_at: now.into(),
            updated_at: (now - chrono::Duration::seconds(updated_secs_ago)).into(),
        }
    }

    #[test]
    fn audio_retention_rules() {
        use pnex_core::media_ingest::AudioRetention::{Days, Keep, None as Never};
        let now = Utc::now();
        // none: waits for the transcription, or the replay window.
        assert!(!audio_expired(&seg("queued", 3600, 3600), Never, now));
        assert!(!audio_expired(&seg("failed", 60, 60), Never, now));
        assert!(audio_expired(&seg("failed", 3600, 3600), Never, now));
        assert!(audio_expired(&seg("captured", 3600, 3600), Never, now));
        // days:N: by age of the audio, whatever the state.
        assert!(!audio_expired(
            &seg("transcribed", 3600, 3600),
            Days(1),
            now
        ));
        assert!(audio_expired(
            &seg("transcribed", 2 * 86_400, 10),
            Days(1),
            now
        ));
        assert!(!audio_expired(
            &seg("transcribed", 400 * 86_400, 10),
            Keep,
            now
        ));
        let mut gone = seg("failed", 3600, 3600);
        gone.storage_key = None;
        assert!(!audio_expired(&gone, Never, now));
    }

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
