//! Media ingest API (media-ingest.md §6, P2.13): streams, their segments
//! and ASR profiles. Org from the principal (R1), `can_write` at the head
//! of every write handler (R2), no secret value in any DTO (R4, R16).

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::Json;
use loco_rs::controller::{format, ErrorDetail};
use loco_rs::prelude::*;
use pnex_core::err_codes;
use pnex_core::media_ingest::{
    AsrModelInput, AsrProfileInput, MediaSegment, MediaStreamInput, SegmentState,
};
use sea_orm::{ColumnTrait, EntityTrait, QueryFilter, QueryOrder};
use serde::Deserialize;
use uuid::Uuid;

use crate::auth::OrgContext;
use crate::controllers::pagination;
use crate::models::_entities::media_segments;
use crate::services::media_ingest::models::{self, ModelError};
use crate::services::media_ingest::profiles::{self, ProfileError};
use crate::services::media_ingest::streams::{self, Author, StreamError};
use crate::services::media_ingest::transcripts;
use crate::services::media_ingest::MediaIngestSettings;

pub fn routes() -> Routes {
    Routes::new()
        .prefix("/api/v1")
        .add("/media/streams", get(stream_list).post(stream_create))
        .add(
            "/media/streams/{id}",
            get(stream_get).patch(stream_update).delete(stream_delete),
        )
        .add("/media/streams/{id}/segments", get(segment_list))
        .add("/media/streams/{id}/segments/retry", post(segment_retry))
        .add("/media/transcripts", get(transcript_search))
        .add("/asr/models", get(model_list).post(model_create))
        .add(
            "/asr/models/{id}",
            get(model_get).patch(model_update).delete(model_delete),
        )
        .add("/asr/models/{id}/check", post(model_check))
        .add("/asr/profiles", get(profile_list).post(profile_create))
        .add(
            "/asr/profiles/{id}",
            get(profile_get)
                .patch(profile_update)
                .delete(profile_delete),
        )
}

fn detail(status: StatusCode, code: &str, msg: &str) -> Error {
    Error::CustomError(status, ErrorDetail::new(code, msg.to_string()))
}

fn require_write(org: &OrgContext) -> Result<()> {
    if org.can_write() {
        Ok(())
    } else {
        Err(detail(
            StatusCode::FORBIDDEN,
            err_codes::MEDIA_STREAM_WRITE_FORBIDDEN,
            "Viewer role: media streams and ASR profiles are read-only.",
        ))
    }
}

fn field(field: &str, token: &str) -> Result<Response> {
    Ok((
        StatusCode::BAD_REQUEST,
        format::json(serde_json::json!({ field: token })),
    )
        .into_response())
}

fn stream_error(e: StreamError) -> Result<Response> {
    match e {
        StreamError::Invalid { field: f, token } => field(f, &token),
        StreamError::NotFound => Err(detail(
            StatusCode::NOT_FOUND,
            err_codes::MEDIA_STREAM_NOT_FOUND,
            "Media stream not found.",
        )),
        StreamError::Limit => Err(detail(
            StatusCode::CONFLICT,
            err_codes::MEDIA_STREAM_LIMIT,
            "Org already has the maximum number of media streams.",
        )),
        StreamError::UrlHasCredentials => Err(detail(
            StatusCode::BAD_REQUEST,
            err_codes::MEDIA_URL_HAS_CREDENTIALS,
            "Stream URL carries a credential; use the stream secret instead.",
        )),
        StreamError::Unreachable => Err(detail(
            StatusCode::BAD_REQUEST,
            err_codes::MEDIA_STREAM_UNREACHABLE,
            "Stream refused or unreachable.",
        )),
        StreamError::CaptureUnsupported => Err(detail(
            StatusCode::BAD_REQUEST,
            err_codes::MEDIA_CAPTURE_UNSUPPORTED,
            "Capture carrier not available yet.",
        )),
        StreamError::AsrModelInvalid => Err(detail(
            StatusCode::CONFLICT,
            err_codes::MEDIA_ASR_MODEL_INVALID,
            "Profile model is not a valid audio model.",
        )),
        StreamError::Store(e) => crate::controllers::secrets::store_error(e),
    }
}

fn profile_error(e: ProfileError) -> Result<Response> {
    match e {
        ProfileError::Invalid { field: f, token } => field(f, &token),
        ProfileError::NotFound => Err(detail(
            StatusCode::NOT_FOUND,
            err_codes::ASR_PROFILE_NOT_FOUND,
            "ASR profile not found.",
        )),
        ProfileError::NameTaken => Err(detail(
            StatusCode::CONFLICT,
            err_codes::ASR_PROFILE_NAME_TAKEN,
            "An ASR profile of the org already has this name.",
        )),
        ProfileError::Db(e) => db_error(e),
    }
}

fn db_error<T>(e: sea_orm::DbErr) -> Result<T> {
    tracing::error!(error = %e, "media ingest database error");
    Err(Error::InternalServerError)
}

/// One stream read model with its secret reference resolved.
async fn one(
    ctx: &AppContext,
    org_id: i64,
    row: &crate::models::_entities::media_streams::Model,
    status: StatusCode,
) -> Result<Response> {
    match streams::views(&ctx.db, org_id, std::slice::from_ref(row)).await {
        Ok(mut v) => Ok((status, format::json(v.remove(0))).into_response()),
        Err(e) => crate::controllers::secrets::store_error(e),
    }
}

fn author(org: &OrgContext) -> Author {
    Author {
        user_id: Some(org.auth.user.id),
        can_manage_secrets: org.can_manage_secrets(),
    }
}

#[derive(Debug, Deserialize)]
struct PageQuery {
    limit: Option<String>,
    offset: Option<String>,
    state: Option<String>,
}

// ───────────────────────────── Streams ─────────────────────────────

/// `GET /api/v1/media/streams` — every member (D14 envelope).
async fn stream_list(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Query(q): Query<PageQuery>,
) -> Result<Response> {
    let page = pagination::PageParams::from(q.limit.as_deref(), q.offset.as_deref());
    let (count, rows) = match pagination::sql_page(&ctx.db, streams::select(org.org.id), page).await
    {
        Ok(v) => v,
        Err(e) => return db_error(e),
    };
    let results = match streams::views(&ctx.db, org.org.id, &rows).await {
        Ok(v) => v,
        Err(e) => return crate::controllers::secrets::store_error(e),
    };
    format::json(pagination::envelope(
        "/api/v1/media/streams",
        &[],
        page,
        count,
        results,
    ))
}

/// `GET /api/v1/media/streams/{id}` — every member.
async fn stream_get(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(id): Path<Uuid>,
) -> Result<Response> {
    match streams::find(&ctx.db, org.org.id, id).await {
        Ok(row) => one(&ctx, org.org.id, &row, StatusCode::OK).await,
        Err(e) => stream_error(e),
    }
}

/// `POST /api/v1/media/streams` — `can_write`; binding a secret needs
/// owner/admin (R9, enforced by the vault).
async fn stream_create(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Json(input): Json<MediaStreamInput>,
) -> Result<Response> {
    require_write(&org)?;
    let ring = match crate::controllers::secrets::keyring(&ctx) {
        Ok(ring) => ring,
        Err(e) => return crate::controllers::secrets::store_error(e),
    };
    let max = MediaIngestSettings::from_config(&ctx.config).max_streams_per_org;
    match streams::create(&ctx.db, &ring, org.org.id, max, author(&org), &input).await {
        Ok(row) => one(&ctx, org.org.id, &row, StatusCode::CREATED).await,
        Err(e) => stream_error(e),
    }
}

/// `PATCH /api/v1/media/streams/{id}` — `can_write`; absent fields keep
/// their value. Moving a secret-holding stream to another origin needs
/// owner/admin (R9).
async fn stream_update(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(id): Path<Uuid>,
    Json(input): Json<MediaStreamInput>,
) -> Result<Response> {
    require_write(&org)?;
    let ring = match crate::controllers::secrets::keyring(&ctx) {
        Ok(ring) => ring,
        Err(e) => return crate::controllers::secrets::store_error(e),
    };
    match streams::update(&ctx.db, &ring, org.org.id, author(&org), id, &input).await {
        Ok(row) => one(&ctx, org.org.id, &row, StatusCode::OK).await,
        Err(e) => stream_error(e),
    }
}

/// `DELETE /api/v1/media/streams/{id}` — `can_write`. Segment rows go with
/// the stream (cascade); blobs still stored are removed best effort.
async fn stream_delete(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(id): Path<Uuid>,
) -> Result<Response> {
    require_write(&org)?;
    let keys: Vec<String> = media_segments::Entity::find()
        .filter(media_segments::Column::OrgId.eq(org.org.id))
        .filter(media_segments::Column::StreamId.eq(id))
        .filter(media_segments::Column::StorageKey.is_not_null())
        .all(&ctx.db)
        .await
        .map_err(|_| Error::InternalServerError)?
        .into_iter()
        .filter_map(|s| s.storage_key)
        .collect();
    if let Err(e) = streams::delete(&ctx.db, org.org.id, id).await {
        return stream_error(e);
    }
    if !keys.is_empty() {
        if let Ok(store) = crate::services::media::MediaSettings::from_config(&ctx.config).store() {
            for key in keys {
                if let Err(e) = store.delete(&key).await {
                    tracing::warn!(error = %e, "media segment blob not deleted");
                }
            }
        }
    }
    Ok(StatusCode::NO_CONTENT.into_response())
}

/// `GET /api/v1/media/streams/{id}/segments` — every member, newest
/// first, optional `state` filter (D14 envelope).
async fn segment_list(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(id): Path<Uuid>,
    Query(q): Query<PageQuery>,
) -> Result<Response> {
    if let Err(e) = streams::find(&ctx.db, org.org.id, id).await {
        return stream_error(e);
    }
    let page = pagination::PageParams::from(q.limit.as_deref(), q.offset.as_deref());
    let mut query = media_segments::Entity::find()
        .filter(media_segments::Column::OrgId.eq(org.org.id))
        .filter(media_segments::Column::StreamId.eq(id))
        .order_by_desc(media_segments::Column::StartedAt);
    let mut filters = Vec::new();
    if let Some(state) = q.state.as_deref().filter(|s| !s.is_empty()) {
        let Some(state) = SegmentState::from_wire(state) else {
            return field("state", err_codes::FIELD_INVALID);
        };
        query = query.filter(media_segments::Column::State.eq(state.wire()));
        filters.push(("state".to_string(), state.wire().to_string()));
    }
    let (count, rows) = match pagination::sql_page(&ctx.db, query, page).await {
        Ok(v) => v,
        Err(e) => return db_error(e),
    };
    let results: Vec<MediaSegment> = rows.iter().map(segment_view).collect();
    format::json(pagination::envelope(
        &format!("/api/v1/media/streams/{id}/segments"),
        &filters,
        page,
        count,
        results,
    ))
}

/// `POST /api/v1/media/streams/{id}/segments/retry` — `can_write`:
/// re-queues the failed segments whose audio is still kept (D161: a failed
/// job is never replayed by the queue itself).
async fn segment_retry(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(id): Path<Uuid>,
) -> Result<Response> {
    require_write(&org)?;
    let stream = match streams::find(&ctx.db, org.org.id, id).await {
        Ok(s) => s,
        Err(e) => return stream_error(e),
    };
    if stream.asr_profile_id.is_none() {
        return stream_error(StreamError::AsrModelInvalid);
    }
    let failed = media_segments::Entity::find()
        .filter(media_segments::Column::OrgId.eq(org.org.id))
        .filter(media_segments::Column::StreamId.eq(id))
        .filter(media_segments::Column::State.eq(SegmentState::Failed.wire()))
        .filter(media_segments::Column::StorageKey.is_not_null())
        .all(&ctx.db)
        .await;
    let failed = match failed {
        Ok(rows) => rows,
        Err(e) => return db_error(e),
    };
    for seg in &failed {
        crate::services::media_ingest::segments::enqueue(&ctx, seg).await;
    }
    format::json(serde_json::json!({ "requeued": failed.len() }))
}

fn segment_view(s: &media_segments::Model) -> MediaSegment {
    MediaSegment {
        id: s.id.to_string(),
        seq: s.seq,
        started_at: s.started_at.to_rfc3339(),
        ended_at: s.ended_at.to_rfc3339(),
        clock_source: s.clock_source.clone(),
        state: SegmentState::from_wire(&s.state).unwrap_or(SegmentState::Failed),
        size_bytes: s.size_bytes,
        asr_model_id: s.asr_model_id.map(|v| v.to_string()),
        asr_ms: s.asr_ms,
        error: s.error.clone(),
    }
}

// ───────────────────────────── Profiles ─────────────────────────────

/// `GET /api/v1/asr/profiles` — every member.
async fn profile_list(State(ctx): State<AppContext>, org: OrgContext) -> Result<Response> {
    match profiles::list(&ctx.db, org.org.id).await {
        Ok(rows) => format::json(rows.iter().map(profiles::view).collect::<Vec<_>>()),
        Err(e) => db_error(e),
    }
}

/// `GET /api/v1/asr/profiles/{id}` — every member.
async fn profile_get(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(id): Path<Uuid>,
) -> Result<Response> {
    match profiles::find(&ctx.db, org.org.id, id).await {
        Ok(row) => format::json(profiles::view(&row)),
        Err(e) => profile_error(e),
    }
}

/// `POST /api/v1/asr/profiles` — `can_write`.
async fn profile_create(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Json(input): Json<AsrProfileInput>,
) -> Result<Response> {
    require_write(&org)?;
    match profiles::create(&ctx.db, org.org.id, &input).await {
        Ok(row) => Ok((StatusCode::CREATED, format::json(profiles::view(&row))).into_response()),
        Err(e) => profile_error(e),
    }
}

/// `PATCH /api/v1/asr/profiles/{id}` — `can_write`.
async fn profile_update(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(id): Path<Uuid>,
    Json(input): Json<AsrProfileInput>,
) -> Result<Response> {
    require_write(&org)?;
    match profiles::update(&ctx.db, org.org.id, id, &input).await {
        Ok(row) => format::json(profiles::view(&row)),
        Err(e) => profile_error(e),
    }
}

/// `DELETE /api/v1/asr/profiles/{id}` — `can_write`.
async fn profile_delete(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(id): Path<Uuid>,
) -> Result<Response> {
    require_write(&org)?;
    match profiles::delete(&ctx.db, org.org.id, id).await {
        Ok(()) => Ok(StatusCode::NO_CONTENT.into_response()),
        Err(e) => profile_error(e),
    }
}

// ───────────────────────────── Audio models ─────────────────────────────

fn model_error(e: ModelError) -> Result<Response> {
    match e {
        ModelError::Invalid { field: f, token } => field(f, &token),
        ModelError::NotFound => Err(detail(
            StatusCode::NOT_FOUND,
            err_codes::ASR_MODEL_NOT_FOUND,
            "Audio model not found.",
        )),
        ModelError::NameTaken => Err(detail(
            StatusCode::CONFLICT,
            err_codes::ASR_MODEL_NAME_TAKEN,
            "A model of the org already has this name.",
        )),
        ModelError::Unsupported => Err(detail(
            StatusCode::BAD_REQUEST,
            err_codes::ASR_MODEL_UNSUPPORTED,
            "The file is not a supported audio model.",
        )),
        ModelError::Db(e) => db_error(e),
    }
}

/// `GET /api/v1/asr/models` — every member.
async fn model_list(State(ctx): State<AppContext>, org: OrgContext) -> Result<Response> {
    match models::list(&ctx.db, org.org.id).await {
        Ok(rows) => format::json(rows.iter().map(models::view).collect::<Vec<_>>()),
        Err(e) => db_error(e),
    }
}

/// `GET /api/v1/asr/models/{id}` — every member.
async fn model_get(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(id): Path<Uuid>,
) -> Result<Response> {
    match models::find(&ctx.db, org.org.id, id).await {
        Ok(row) => format::json(models::view(&row)),
        Err(e) => model_error(e),
    }
}

/// `POST /api/v1/asr/models` — `can_write`. The files are read and the
/// model checked before the answer (family read, never typed).
async fn model_create(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Json(input): Json<AsrModelInput>,
) -> Result<Response> {
    require_write(&org)?;
    match models::create(&ctx, org.org.id, &input).await {
        Ok(row) => Ok((StatusCode::CREATED, format::json(models::view(&row))).into_response()),
        Err(e) => model_error(e),
    }
}

/// `PATCH /api/v1/asr/models/{id}` — `can_write` (name, description,
/// license; the file never changes).
async fn model_update(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(id): Path<Uuid>,
    Json(input): Json<AsrModelInput>,
) -> Result<Response> {
    require_write(&org)?;
    match models::update(&ctx, org.org.id, id, &input).await {
        Ok(row) => format::json(models::view(&row)),
        Err(e) => model_error(e),
    }
}

/// `DELETE /api/v1/asr/models/{id}` — `can_write`.
async fn model_delete(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(id): Path<Uuid>,
) -> Result<Response> {
    require_write(&org)?;
    match models::delete(&ctx, org.org.id, id).await {
        Ok(()) => Ok(StatusCode::NO_CONTENT.into_response()),
        Err(e) => model_error(e),
    }
}

/// `POST /api/v1/asr/models/{id}/check` — `can_write`: reload the model and
/// transcribe the reference sample on this server.
async fn model_check(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(id): Path<Uuid>,
) -> Result<Response> {
    require_write(&org)?;
    let row = match models::find(&ctx.db, org.org.id, id).await {
        Ok(row) => row,
        Err(e) => return model_error(e),
    };
    match models::run_check(&ctx, row).await {
        Ok(row) => format::json(models::view(&row)),
        Err(e) => model_error(e),
    }
}

// ───────────────────────────── Transcripts ─────────────────────────────

#[derive(Debug, Deserialize)]
struct TranscriptQuery {
    /// Comma-separated slugs; empty = every stream of the org.
    stream: Option<String>,
    q: Option<String>,
    /// RFC 3339 bounds.
    from: Option<String>,
    to: Option<String>,
    limit: Option<String>,
    offset: Option<String>,
}

fn parse_time(
    field: &'static str,
    raw: Option<&str>,
) -> std::result::Result<Option<i64>, &'static str> {
    match raw.map(str::trim).filter(|s| !s.is_empty()) {
        None => Ok(None),
        Some(s) => chrono::DateTime::parse_from_rfc3339(s)
            .map(|t| Some(t.timestamp_micros()))
            .map_err(|_| field),
    }
}

/// `GET /api/v1/media/transcripts` — every member. Slugs are resolved to
/// stream rows of the org; O2 stream names are built server side (D165).
async fn transcript_search(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Query(q): Query<TranscriptQuery>,
) -> Result<Response> {
    let page = pagination::PageParams::from(q.limit.as_deref(), q.offset.as_deref());
    let text = q.q.as_deref().map(str::trim).filter(|t| !t.is_empty());
    if text.is_some_and(|t| t.chars().count() > transcripts::QUERY_MAX) {
        return field(
            "q",
            &format!("{}:{}", err_codes::FIELD_MAX_LENGTH, transcripts::QUERY_MAX),
        );
    }
    let (from_us, to_us) = match (
        parse_time("from", q.from.as_deref()),
        parse_time("to", q.to.as_deref()),
    ) {
        (Ok(f), Ok(t)) => (f, t),
        (Err(f), _) | (_, Err(f)) => return field(f, err_codes::FIELD_INVALID),
    };
    let rows = match streams::select(org.org.id).all(&ctx.db).await {
        Ok(rows) => rows,
        Err(e) => return db_error(e),
    };
    let wanted: Vec<String> = q
        .stream
        .as_deref()
        .unwrap_or_default()
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect();
    let slugs: Vec<String> = if wanted.is_empty() {
        rows.iter().map(|r| r.slug.clone()).collect()
    } else {
        // Unknown slugs: the hidden 404 of a stream that is not the org's.
        for w in &wanted {
            if !rows.iter().any(|r| &r.slug == w) {
                return stream_error(StreamError::NotFound);
            }
        }
        wanted.clone()
    };
    let query = transcripts::Query {
        slugs,
        text: text.map(str::to_string),
        from_us,
        to_us,
        offset: page.offset,
        limit: page.limit,
    };
    match transcripts::search(&ctx, org.org.id, &query).await {
        Ok((count, results)) => {
            let mut filters = Vec::new();
            if !wanted.is_empty() {
                filters.push(("stream".to_string(), wanted.join(",")));
            }
            if let Some(t) = text {
                filters.push(("q".to_string(), t.to_string()));
            }
            format::json(pagination::envelope(
                "/api/v1/media/transcripts",
                &filters,
                page,
                count,
                results,
            ))
        }
        Err(e) => {
            tracing::warn!(error = %e, "transcript search failed");
            Err(detail(
                StatusCode::BAD_GATEWAY,
                err_codes::MEDIA_TRANSCRIPTS_UNAVAILABLE,
                "Transcription search failed on OpenObserve.",
            ))
        }
    }
}
