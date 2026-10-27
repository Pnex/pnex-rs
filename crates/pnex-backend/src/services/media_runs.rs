//! Transcription runs of a media asset (media-review.md D276): a run is a
//! small row here (origin, version, language), its text lives in O2 in the
//! `tx_<slug>` stream of the asset's hidden batch stream, each document
//! tagged with `run`, `origin` and `offset_ms` (position in the media).
//!
//! - `pnex` runs come from the batch transcription of a version (D211).
//! - `platform` runs are the subtitles a platform gave with a post
//!   (browser agents, `extension-collector.md`): no speakers, timed per
//!   cue, stored by [`store_platform_run`], possibly before any video file.
//!
//! The org always comes from the caller (R1); subtitle text is untrusted
//! data, stored as is, never rendered as HTML.

use loco_rs::app::AppContext;
use pnex_core::media_review::{default_run, shift_segment, MediaRun, MediaTranscript, RunOrigin};
use sea_orm::{
    ColumnTrait, ConnectionTrait, DatabaseConnection, DbErr, EntityTrait, FromQueryResult,
    QueryFilter, Statement,
};
use uuid::Uuid;

use crate::models::_entities::media_assets;
use crate::services::media_ingest::{streams, transcripts};

/// Most cues of one platform run.
pub const PLATFORM_CUES_MAX: usize = 20_000;
/// Longest text of one cue.
pub const CUE_TEXT_MAX: usize = 2_000;
/// Latest position of a cue: a day of media.
const CUE_END_MAX_MS: i64 = 24 * 3600 * 1000;

/// One subtitle cue of a platform, ms from the start of the media.
#[derive(Debug, Clone, PartialEq)]
pub struct PlatformCue {
    pub start_ms: i64,
    pub end_ms: i64,
    pub text: String,
}

#[derive(Debug, thiserror::Error)]
pub enum RunError {
    #[error("unknown media asset")]
    NotFound,
    #[error("invalid subtitles: {0}")]
    Invalid(&'static str),
    #[error("text store: {0}")]
    Store(String),
    #[error(transparent)]
    Db(#[from] DbErr),
}

fn stmt(sql: &str, values: Vec<sea_orm::Value>) -> Statement {
    Statement::from_sql_and_values(sea_orm::DbBackend::Postgres, sql, values)
}

/// Records a run (its id is the `run` of its documents).
#[allow(clippy::too_many_arguments)]
pub async fn record<C: ConnectionTrait>(
    db: &C,
    id: Uuid,
    org_id: i64,
    asset_id: Uuid,
    version_id: Option<Uuid>,
    origin: RunOrigin,
    lang: Option<&str>,
    user_id: Option<i64>,
) -> Result<(), DbErr> {
    db.execute_raw(stmt(
        "INSERT INTO media_runs (id, org_id, media_asset_id, media_version_id, origin, lang,
            created_by) VALUES ($1, $2, $3, $4, $5, $6, $7)",
        vec![
            id.into(),
            org_id.into(),
            asset_id.into(),
            version_id.into(),
            origin.wire().into(),
            lang.map(str::to_string).into(),
            user_id.into(),
        ],
    ))
    .await?;
    Ok(())
}

/// Clock base of a run: its creation date. The documents of a run sit at
/// `base + offset_ms` (O2 only takes documents of the last hours, so the
/// base is the run date, never the version date; the position in the media
/// is `offset_ms`).
pub async fn run_base<C: ConnectionTrait>(
    db: &C,
    run: Uuid,
) -> Result<Option<chrono::DateTime<chrono::Utc>>, DbErr> {
    #[derive(FromQueryResult)]
    struct B {
        created_at: chrono::DateTime<chrono::FixedOffset>,
    }
    Ok(B::find_by_statement(stmt(
        "SELECT created_at FROM media_runs WHERE id = $1",
        vec![run.into()],
    ))
    .one(db)
    .await?
    .map(|b| b.created_at.with_timezone(&chrono::Utc)))
}

#[derive(Debug, FromQueryResult)]
struct RunRow {
    id: Uuid,
    media_version_id: Option<Uuid>,
    origin: String,
    lang: Option<String>,
    created_at: chrono::DateTime<chrono::FixedOffset>,
}

/// Runs of an asset of the org, newest first.
pub async fn list(
    db: &DatabaseConnection,
    org_id: i64,
    asset_id: Uuid,
) -> Result<Vec<MediaRun>, DbErr> {
    let rows = RunRow::find_by_statement(stmt(
        "SELECT id, media_version_id, origin, lang, created_at FROM media_runs
         WHERE org_id = $1 AND media_asset_id = $2 ORDER BY created_at DESC",
        vec![org_id.into(), asset_id.into()],
    ))
    .all(db)
    .await?;
    Ok(rows
        .into_iter()
        .filter_map(|r| {
            Some(MediaRun {
                id: r.id,
                origin: RunOrigin::from_wire(&r.origin)?,
                version_id: r.media_version_id,
                lang: r.lang,
                created_at: r.created_at.to_rfc3339(),
            })
        })
        .collect())
}

async fn asset_of(
    db: &DatabaseConnection,
    org_id: i64,
    asset_id: Uuid,
) -> Result<media_assets::Model, RunError> {
    media_assets::Entity::find_by_id(asset_id)
        .filter(media_assets::Column::OrgId.eq(org_id))
        .one(db)
        .await?
        .ok_or(RunError::NotFound)
}

fn check_cues(lang: &str, cues: &[PlatformCue]) -> Result<(), RunError> {
    if cues.is_empty() || cues.len() > PLATFORM_CUES_MAX {
        return Err(RunError::Invalid("cue count"));
    }
    if lang.is_empty()
        || lang.len() > 16
        || !lang.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
    {
        return Err(RunError::Invalid("lang"));
    }
    for c in cues {
        if c.start_ms < 0 || c.end_ms < c.start_ms || c.end_ms > CUE_END_MAX_MS {
            return Err(RunError::Invalid("cue time"));
        }
        if c.text.chars().count() > CUE_TEXT_MAX {
            return Err(RunError::Invalid("cue text"));
        }
    }
    Ok(())
}

/// Stores the subtitles a platform gave with a post as a `platform` run of
/// the asset (D276): written to O2 like a PNeX run, on the run clock
/// (position 0 = the run date, see [`run_base`]). The asset may have no video
/// file yet. Returns the run.
pub async fn store_platform_run(
    ctx: &AppContext,
    org_id: i64,
    asset_id: Uuid,
    lang: &str,
    cues: &[PlatformCue],
) -> Result<Uuid, RunError> {
    check_cues(lang, cues)?;
    let asset = asset_of(&ctx.db, org_id, asset_id).await?;
    let stream = streams::batch_stream(&ctx.db, org_id, asset.id, None)
        .await
        .map_err(|e| RunError::Store(e.to_string()))?;
    let run = Uuid::new_v4();
    record(
        &ctx.db,
        run,
        org_id,
        asset.id,
        None,
        RunOrigin::Platform,
        Some(lang),
        None,
    )
    .await?;
    let base_us = run_base(&ctx.db, run)
        .await?
        .unwrap_or_else(chrono::Utc::now)
        .timestamp_micros();
    let docs: Vec<transcripts::Doc<'_>> = cues
        .iter()
        .filter(|c| !c.text.trim().is_empty())
        .map(|c| transcripts::Doc {
            started_at_us: base_us + c.start_ms * 1000,
            ended_at_us: base_us + c.end_ms * 1000,
            slug: &stream.slug,
            segment_id: Uuid::new_v4(),
            text: &c.text,
            words_json: "[]".into(),
            speakers_json: "[]".into(),
            speech_ms: None,
            lang,
            asr_model: "platform",
            asr_model_version: None,
            file: Some(transcripts::FilePos {
                offset_ms: c.start_ms,
                run,
                origin: RunOrigin::Platform,
                version: None,
            }),
        })
        .collect();
    if let Err(e) = transcripts::write_many(ctx, org_id, &stream.slug, &docs).await {
        // Nothing stored: the run is not kept either.
        ctx.db
            .execute_raw(stmt(
                "DELETE FROM media_runs WHERE id = $1",
                vec![run.into()],
            ))
            .await?;
        return Err(RunError::Store(e));
    }
    Ok(run)
}

/// Transcript of an asset of the org for the review screen: its runs, the
/// one asked (else [`default_run`]) and its segments in media order.
pub async fn transcript(
    ctx: &AppContext,
    org_id: i64,
    asset_id: Uuid,
    run: Option<Uuid>,
) -> Result<MediaTranscript, RunError> {
    let asset = asset_of(&ctx.db, org_id, asset_id).await?;
    let runs = list(&ctx.db, org_id, asset.id).await?;
    let chosen = match run {
        Some(id) if runs.iter().any(|r| r.id == id) => Some(id),
        Some(_) => return Err(RunError::NotFound),
        None => default_run(&runs, asset.current_version_id),
    };
    let mut out = MediaTranscript {
        runs: runs.clone(),
        run: chosen,
        segments: Vec::new(),
        unavailable: false,
        attributions: crate::services::media_attributions::list(&ctx.db, org_id, asset.id).await?,
    };
    let Some(run) = chosen.and_then(|id| runs.iter().find(|r| r.id == id)) else {
        return Ok(out);
    };
    let stream = crate::models::_entities::media_streams::Entity::find()
        .filter(crate::models::_entities::media_streams::Column::OrgId.eq(org_id))
        .filter(crate::models::_entities::media_streams::Column::MediaAssetId.eq(asset.id))
        .one(&ctx.db)
        .await?;
    let Some(stream) = stream else {
        return Ok(out);
    };
    // The documents sit at `run date + offset` (see `run_base`).
    let base = run_base(&ctx.db, run.id)
        .await?
        .unwrap_or_else(|| asset.created_at.with_timezone(&chrono::Utc));
    let from = base.timestamp_micros() - 60_000_000;
    let to = from + (CUE_END_MAX_MS + 3_600_000) * 1000;
    match transcripts::run_documents(ctx, org_id, &stream.slug, run.id, from, to).await {
        Ok(Some(docs)) => {
            out.segments = docs
                .into_iter()
                .map(|(offset, dur, text, words, turns)| {
                    shift_segment(offset, dur, text, &words, &turns)
                })
                .collect();
        }
        Ok(None) => out.unavailable = true,
        Err(e) => {
            tracing::warn!(asset = %asset.id, error = %e, "transcript not read");
            out.unavailable = true;
        }
    }
    Ok(out)
}

/// A window of a live stream of the org for the review screen (D271 V2):
/// the transcripts on the window clock and the segments whose audio is
/// still kept. `from`..`to` at most [`STREAM_REVIEW_MAX_SECS`].
pub async fn stream_review(
    ctx: &AppContext,
    org_id: i64,
    stream_id: Uuid,
    from: chrono::DateTime<chrono::Utc>,
    to: chrono::DateTime<chrono::Utc>,
) -> Result<pnex_core::media_review::StreamReview, RunError> {
    use crate::models::_entities::media_segments;
    use pnex_core::media_review::{AudioPiece, StreamReview, STREAM_REVIEW_MAX_SECS};
    use sea_orm::QueryOrder;
    if to <= from || (to - from).num_seconds() > STREAM_REVIEW_MAX_SECS {
        return Err(RunError::Invalid("window"));
    }
    let stream = streams::find(&ctx.db, org_id, stream_id)
        .await
        .map_err(|_| RunError::NotFound)?;
    let from_ms = from.timestamp_millis();
    let pieces: Vec<AudioPiece> = media_segments::Entity::find()
        .filter(media_segments::Column::OrgId.eq(org_id))
        .filter(media_segments::Column::StreamId.eq(stream.id))
        .filter(media_segments::Column::StorageKey.is_not_null())
        .filter(media_segments::Column::EndedAt.gt(from))
        .filter(media_segments::Column::StartedAt.lt(to))
        .order_by_asc(media_segments::Column::StartedAt)
        .all(&ctx.db)
        .await?
        .into_iter()
        .map(|s| AudioPiece {
            segment_id: s.id,
            start_ms: s.started_at.timestamp_millis() - from_ms,
            end_ms: s.ended_at.timestamp_millis() - from_ms,
        })
        .collect();
    let mut out = StreamReview {
        from: from.to_rfc3339(),
        to: to.to_rfc3339(),
        segments: Vec::new(),
        audio: pieces,
        unavailable: false,
    };
    match transcripts::window_documents(
        ctx,
        org_id,
        &stream.slug,
        from.timestamp_micros(),
        to.timestamp_micros(),
    )
    .await
    {
        Ok(Some(docs)) => {
            out.segments = docs
                .into_iter()
                .map(|(start_us, end_us, text, words, turns)| {
                    shift_segment(
                        start_us / 1000 - from_ms,
                        (end_us - start_us) / 1000,
                        text,
                        &words,
                        &turns,
                    )
                })
                .collect();
        }
        Ok(None) => out.unavailable = true,
        Err(e) => {
            tracing::warn!(stream = %stream.slug, error = %e, "stream transcript not read");
            out.unavailable = true;
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cue(start_ms: i64, end_ms: i64, text: &str) -> PlatformCue {
        PlatformCue {
            start_ms,
            end_ms,
            text: text.into(),
        }
    }

    #[test]
    fn cues_are_bounded() {
        assert!(check_cues("fr", &[cue(0, 1_000, "bonjour")]).is_ok());
        assert!(check_cues("fr", &[]).is_err());
        assert!(check_cues("fr", &[cue(2_000, 1_000, "x")]).is_err());
        assert!(check_cues("fr", &[cue(-1, 1_000, "x")]).is_err());
        assert!(check_cues("fr'; drop", &[cue(0, 1, "x")]).is_err());
        assert!(check_cues("fr", &[cue(0, 1, &"a".repeat(CUE_TEXT_MAX + 1))]).is_err());
    }
}
