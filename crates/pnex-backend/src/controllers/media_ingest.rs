//! Media ingest API (media-ingest.md §6, P2.13): streams, their segments
//! and ASR profiles. Org from the principal (R1), `can_write` at the head
//! of every write handler (R2), no secret value in any DTO (R4, R16).

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::routing::get;
use axum::Json;
use loco_rs::controller::{format, ErrorDetail};
use loco_rs::prelude::*;
use pnex_core::err_codes;
use pnex_core::media_ingest::{AsrProfileInput, MediaSegment, MediaStreamInput, SegmentState};
use sea_orm::{ColumnTrait, EntityTrait, QueryFilter, QueryOrder};
use serde::Deserialize;
use uuid::Uuid;

use crate::auth::OrgContext;
use crate::controllers::pagination;
use crate::models::_entities::media_segments;
use crate::services::media_ingest::profiles::{self, ProfileError};
use crate::services::media_ingest::streams::{self, Author, StreamError};
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
