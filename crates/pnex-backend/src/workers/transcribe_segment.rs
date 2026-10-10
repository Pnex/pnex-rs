//! `transcribe_segment` (media-ingest.md D166): one captured segment →
//! text in O2 `tx_<slug>` → event on the stream's Valkey channel → audio
//! purged per the stream retention (D161).
//!
//! Patterned on `stitch_panorama`: tiny arguments, idempotent claim in the
//! domain row (`queued → transcribing`, stale `transcribing` taken over),
//! terminal state written by the worker only, a failure is `failed` with a
//! short code and never replayed by the queue (Loco `failed` jobs are
//! terminal). The queue tag is the platform setting `PNEX_ASR_QUEUE_TAG`
//! (empty = untagged queue shared with the other jobs).

use async_trait::async_trait;
use loco_rs::app::AppContext;
use loco_rs::bgworker::BackgroundWorker;
use loco_rs::prelude::*;
use pnex_core::media_ingest::{transcript_channel, AudioRetention, SegmentState};
use pnex_core::vision::ModelCheckStatus;
use sea_orm::sea_query::Expr;
use sea_orm::{ColumnTrait, EntityTrait, QueryFilter};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::models::_entities::{asr_profiles, media_segments, media_streams, ml_models};
use crate::services::media::MediaSettings;
use crate::services::media_ingest::asr::{self, process, AsrSettings, ASR_TASK};
use crate::services::media_ingest::transcripts;

/// A `transcribing` claim older than this is taken over (worker died).
const STALE_CLAIM_SECS: i64 = 20 * 60;
/// Segments older than this when their turn comes are skipped
/// (`skipped_backlog`): the queue must not diverge from live (D166).
const DEFAULT_MAX_LAG_SECS: i64 = 600;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TranscribeSegmentArgs {
    pub segment_id: Uuid,
    pub org_id: i64,
}

pub struct TranscribeSegmentWorker {
    ctx: AppContext,
}

/// Short failure codes stored in `media_segments.error` (no path, no
/// detail: the logs carry it).
#[derive(Debug, Clone, Copy)]
enum Failure {
    NoProfile,
    ModelInvalid,
    AudioMissing,
    Runtime,
    Store,
}

impl Failure {
    fn code(self) -> &'static str {
        match self {
            Self::NoProfile => "no-profile",
            Self::ModelInvalid => "model-invalid",
            Self::AudioMissing => "audio-missing",
            Self::Runtime => "asr-failed",
            Self::Store => "o2-failed",
        }
    }
}

/// Concurrent transcriptions of one org in this process (D166 fairness).
const DEFAULT_MAX_PER_ORG: usize = 2;
/// Pause before a job of a saturated org goes back to the queue.
const REQUEUE_AFTER: std::time::Duration = std::time::Duration::from_secs(2);

fn max_per_org() -> usize {
    std::env::var("PNEX_ASR_MAX_PER_ORG")
        .ok()
        .and_then(|v| v.trim().parse().ok())
        .filter(|n| *n > 0)
        .unwrap_or(DEFAULT_MAX_PER_ORG)
}

static ORG_SLOTS: std::sync::LazyLock<
    std::sync::Mutex<std::collections::HashMap<i64, std::sync::Arc<tokio::sync::Semaphore>>>,
> = std::sync::LazyLock::new(Default::default);

/// A transcription slot of `org_id`, `None` when the org already runs
/// `max` transcriptions in this process: one org never fills every queue
/// worker while the others wait.
fn org_slot(org_id: i64, max: usize) -> Option<tokio::sync::OwnedSemaphorePermit> {
    let sem = ORG_SLOTS
        .lock()
        .expect("asr org slots")
        .entry(org_id)
        .or_insert_with(|| std::sync::Arc::new(tokio::sync::Semaphore::new(max)))
        .clone();
    sem.try_acquire_owned().ok()
}

fn max_lag_secs() -> i64 {
    std::env::var("PNEX_ASR_MAX_LAG_SECS")
        .ok()
        .and_then(|v| v.trim().parse().ok())
        .unwrap_or(DEFAULT_MAX_LAG_SECS)
}

async fn set_state(
    db: &sea_orm::DatabaseConnection,
    id: Uuid,
    state: SegmentState,
    error: Option<&str>,
) -> Result<()> {
    let now: sea_orm::prelude::DateTimeWithTimeZone = chrono::Utc::now().into();
    media_segments::Entity::update_many()
        .col_expr(media_segments::Column::State, Expr::value(state.wire()))
        .col_expr(
            media_segments::Column::Error,
            Expr::value(error.map(str::to_string)),
        )
        .col_expr(media_segments::Column::UpdatedAt, Expr::value(now))
        .filter(media_segments::Column::Id.eq(id))
        .exec(db)
        .await?;
    Ok(())
}

impl TranscribeSegmentWorker {
    async fn claim(db: &sea_orm::DatabaseConnection, id: Uuid, org_id: i64) -> Result<bool> {
        let now = chrono::Utc::now();
        let stale: sea_orm::prelude::DateTimeWithTimeZone =
            (now - chrono::Duration::seconds(STALE_CLAIM_SECS)).into();
        let now: sea_orm::prelude::DateTimeWithTimeZone = now.into();
        let res = media_segments::Entity::update_many()
            .col_expr(
                media_segments::Column::State,
                Expr::value(SegmentState::Transcribing.wire()),
            )
            .col_expr(media_segments::Column::UpdatedAt, Expr::value(now))
            .filter(media_segments::Column::Id.eq(id))
            .filter(media_segments::Column::OrgId.eq(org_id))
            .filter(
                sea_orm::Condition::any()
                    .add(media_segments::Column::State.eq(SegmentState::Queued.wire()))
                    .add(
                        sea_orm::Condition::all()
                            .add(
                                media_segments::Column::State.eq(SegmentState::Transcribing.wire()),
                            )
                            .add(media_segments::Column::UpdatedAt.lt(stale)),
                    ),
            )
            .exec(db)
            .await?;
        Ok(res.rows_affected == 1)
    }

    /// Deletes the blob when the stream keeps no audio (D161).
    async fn purge(&self, seg: &media_segments::Model, stream: &media_streams::Model) {
        let keep = !matches!(
            AudioRetention::from_wire(&stream.audio_retention),
            Some(AudioRetention::None) | None
        );
        let Some(key) = seg.storage_key.as_deref() else {
            return;
        };
        if keep {
            return;
        }
        if let Ok(store) = MediaSettings::from_config(&self.ctx.config).store() {
            if let Err(e) = store.delete(key).await {
                tracing::warn!(segment = %seg.id, error = %e, "segment audio not purged");
                return;
            }
            let _ = media_segments::Entity::update_many()
                .col_expr(
                    media_segments::Column::StorageKey,
                    Expr::value(Option::<String>::None),
                )
                .filter(media_segments::Column::Id.eq(seg.id))
                .exec(&self.ctx.db)
                .await;
        }
    }

    async fn run(&self, seg: &media_segments::Model) -> std::result::Result<(), Failure> {
        let db = &self.ctx.db;
        let stream = media_streams::Entity::find_by_id(seg.stream_id)
            .filter(media_streams::Column::OrgId.eq(seg.org_id))
            .one(db)
            .await
            .map_err(|_| Failure::Store)?
            .ok_or(Failure::NoProfile)?;
        let profile = match stream.asr_profile_id {
            Some(p) => asr_profiles::Entity::find_by_id(p)
                .filter(asr_profiles::Column::OrgId.eq(seg.org_id))
                .one(db)
                .await
                .map_err(|_| Failure::Store)?,
            None => None,
        }
        .ok_or(Failure::NoProfile)?;
        let model = ml_models::Entity::find_by_id(profile.asr_model_id)
            .filter(ml_models::Column::OrgId.eq(seg.org_id))
            .filter(ml_models::Column::Task.eq(ASR_TASK))
            .one(db)
            .await
            .map_err(|_| Failure::Store)?
            .filter(|m| m.check_status == ModelCheckStatus::Valid.wire())
            .ok_or(Failure::ModelInvalid)?;

        let key = seg.storage_key.as_deref().ok_or(Failure::AudioMissing)?;
        let store = MediaSettings::from_config(&self.ctx.config)
            .store()
            .map_err(|_| Failure::AudioMissing)?;
        let wav = store.get(key).await.map_err(|_| Failure::AudioMissing)?;

        let settings = AsrSettings::from_env();
        let local = asr::local_model(&self.ctx, &settings, model.asset_id, model.asset_version)
            .await
            .map_err(|e| {
                tracing::warn!(model = %model.id, error = %e, "audio model unavailable");
                Failure::ModelInvalid
            })?;
        let family =
            pnex_asr::protocol::Family::from_wire(&model.family).unwrap_or(local.inspection.family);
        let launch = asr::launch(&settings, &local, family, &profile.language).map_err(|e| {
            tracing::warn!(error = %e, "pnex-asr unavailable");
            Failure::Runtime
        })?;
        let resp = process::transcribe(&launch, &settings.sandbox, &wav)
            .await
            .map_err(|e| {
                tracing::warn!(segment = %seg.id, error = %e, "transcription failed");
                Failure::Runtime
            })?;
        let text = resp.text.clone().unwrap_or_default();

        if !text.trim().is_empty() {
            let words_json = serde_json::to_string(&resp.words).unwrap_or_default();
            let doc = transcripts::Doc {
                started_at_us: seg.started_at.timestamp_micros(),
                ended_at_us: seg.ended_at.timestamp_micros(),
                slug: &stream.slug,
                segment_id: seg.id,
                text: &text,
                words_json,
                lang: &profile.language,
                asr_model: &model.name,
                asr_model_version: model.asset_version,
            };
            transcripts::write(&self.ctx, seg.org_id, &doc)
                .await
                .map_err(|e| {
                    tracing::warn!(segment = %seg.id, error = %e, "transcription not stored");
                    Failure::Store
                })?;
            self.publish(seg, &stream, &profile, &model, &text).await;
        }

        let now: sea_orm::prelude::DateTimeWithTimeZone = chrono::Utc::now().into();
        media_segments::Entity::update_many()
            .col_expr(
                media_segments::Column::State,
                Expr::value(SegmentState::Transcribed.wire()),
            )
            .col_expr(
                media_segments::Column::Error,
                Expr::value(Option::<String>::None),
            )
            .col_expr(
                media_segments::Column::AsrModelId,
                Expr::value(Some(model.id)),
            )
            .col_expr(
                media_segments::Column::AsrModelVersion,
                Expr::value(model.asset_version),
            )
            .col_expr(
                media_segments::Column::AsrMs,
                Expr::value(Some(resp.ms as i64)),
            )
            .col_expr(media_segments::Column::UpdatedAt, Expr::value(now))
            .filter(media_segments::Column::Id.eq(seg.id))
            .exec(db)
            .await
            .map_err(|_| Failure::Store)?;
        self.purge(seg, &stream).await;
        Ok(())
    }

    /// Segment event for `media_source` (D163), best effort: the text is
    /// already in O2. The channel is built from the row's org and slug (R3).
    async fn publish(
        &self,
        seg: &media_segments::Model,
        stream: &media_streams::Model,
        profile: &asr_profiles::Model,
        model: &ml_models::Model,
        text: &str,
    ) {
        let Some(mut conn) = crate::services::shared_valkey::conn(&self.ctx.config).await else {
            return;
        };
        let event = serde_json::json!({
            "stream": stream.slug,
            "segment_id": seg.id.to_string(),
            "started_at": seg.started_at.to_rfc3339(),
            "ended_at": seg.ended_at.to_rfc3339(),
            "text": text,
            "lang": profile.language,
            "asr_model": model.name,
        });
        let channel = transcript_channel(seg.org_id, &stream.slug);
        let res: redis::RedisResult<i64> = redis::cmd("PUBLISH")
            .arg(&channel)
            .arg(event.to_string())
            .query_async(&mut conn)
            .await;
        if let Err(e) = res {
            tracing::debug!(error = %e, "segment event not published");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn org_slots_cap_one_org_only() {
        let a1 = org_slot(-11, 2);
        let a2 = org_slot(-11, 2);
        assert!(a1.is_some() && a2.is_some());
        assert!(org_slot(-11, 2).is_none(), "third slot of the org refused");
        assert!(org_slot(-12, 2).is_some(), "another org is not affected");
        drop(a1);
        assert!(
            org_slot(-11, 2).is_some(),
            "a slot comes back when released"
        );
    }
}

#[async_trait]
impl BackgroundWorker<TranscribeSegmentArgs> for TranscribeSegmentWorker {
    fn build(ctx: &AppContext) -> Self {
        Self { ctx: ctx.clone() }
    }

    fn tags() -> Vec<String> {
        crate::services::media_ingest::asr_queue_tags()
    }

    async fn perform(&self, args: TranscribeSegmentArgs) -> Result<()> {
        let Some(seg) = media_segments::Entity::find_by_id(args.segment_id)
            .filter(media_segments::Column::OrgId.eq(args.org_id))
            .one(&self.ctx.db)
            .await?
        else {
            return Ok(()); // stream deleted meanwhile
        };
        let Some(_slot) = org_slot(args.org_id, max_per_org()) else {
            // Back to the queue: the worker slot goes to another org's job.
            tokio::time::sleep(REQUEUE_AFTER).await;
            let live = seg.state == SegmentState::Queued.wire();
            Self::perform_later_with_priority(
                &self.ctx,
                args,
                crate::services::media_ingest::segments::priority(live),
            )
            .await?;
            return Ok(());
        };
        if !Self::claim(&self.ctx.db, seg.id, args.org_id).await? {
            tracing::debug!(segment = %seg.id, state = %seg.state, "segment already handled");
            return Ok(());
        }
        let lag = chrono::Utc::now().signed_duration_since(seg.ended_at);
        if lag.num_seconds() > max_lag_secs() {
            set_state(&self.ctx.db, seg.id, SegmentState::SkippedBacklog, None).await?;
            if let Some(stream) = media_streams::Entity::find_by_id(seg.stream_id)
                .one(&self.ctx.db)
                .await?
            {
                self.purge(&seg, &stream).await;
            }
            return Ok(());
        }
        if let Err(f) = self.run(&seg).await {
            // The blob stays for an explicit replay (D161).
            set_state(&self.ctx.db, seg.id, SegmentState::Failed, Some(f.code())).await?;
        }
        Ok(())
    }
}
