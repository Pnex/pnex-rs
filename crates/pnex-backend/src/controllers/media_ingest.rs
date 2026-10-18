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
        .add("/media/streams/{id}/test", post(stream_test))
        .add("/media/capture-devices", get(capture_device_list))
        .add("/media/streams/{id}/segments", get(segment_list))
        .add("/media/streams/{id}/segments/retry", post(segment_retry))
        .add("/media/transcripts", get(transcript_search))
        .add("/media/metadata", get(metadata_list))
        .add("/asr/models", get(model_list).post(model_create))
        .add(
            "/asr/models/{id}",
            get(model_get).patch(model_update).delete(model_delete),
        )
        .add("/asr/models/{id}/check", post(model_check))
        .add(
            "/asr/models/{id}/test",
            post(model_test).layer(axum::extract::DefaultBodyLimit::max(TEST_CLIP_MAX_BYTES)),
        )
        .add("/asr/profiles", get(profile_list).post(profile_create))
        .add(
            "/asr/profiles/{id}",
            get(profile_get)
                .patch(profile_update)
                .delete(profile_delete),
        )
}

/// OpenAPI paths of this controller (merged by `crate::openapi::spec`).
pub fn openapi() -> utoipa::openapi::OpenApi {
    #[derive(utoipa::OpenApi)]
    #[openapi(paths(
        stream_list,
        stream_create,
        stream_get,
        stream_update,
        stream_delete,
        stream_test,
        capture_device_list,
        segment_list,
        segment_retry,
        transcript_search,
        metadata_list,
        model_list,
        model_create,
        model_get,
        model_update,
        model_delete,
        model_check,
        model_test,
        profile_list,
        profile_create,
        profile_get,
        profile_update,
        profile_delete
    ))]
    struct Doc;
    <Doc as utoipa::OpenApi>::openapi()
}

/// Segments uploaded by mesh workers (D160, `capture_on = worker`).
/// Service token, never routed by the public edge (R7, internal guard).
pub fn internal_routes() -> Routes {
    Routes::new().add(
        "/internal/media/segment",
        post(internal_segment).layer(axum::extract::DefaultBodyLimit::max(SEGMENT_BODY_MAX)),
    )
}

/// Largest possible segment: `(120 + 3) s` of 16 kHz mono s16 + header.
const SEGMENT_BODY_MAX: usize = 4 * 1024 * 1024;

#[derive(Debug, Deserialize)]
pub struct SegmentUpload {
    stream_id: Uuid,
    org_id: i64,
    seq: i64,
    started_ms: i64,
    ended_ms: i64,
    clock: String,
}

/// `POST /internal/media/segment` — a WAV segment captured by a mesh
/// worker. The stream must belong to `org_id` and be captured by a worker;
/// a re-sent `seq` is acknowledged without a second row.
async fn internal_segment(
    State(ctx): State<AppContext>,
    headers: axum::http::HeaderMap,
    Query(q): Query<SegmentUpload>,
    body: axum::body::Bytes,
) -> Result<Response> {
    use crate::models::_entities::media_streams;
    use crate::services::media_ingest::segments::{self, UploadError};
    use pnex_core::media_ingest::{CaptureOn, ClockSource};
    if !crate::controllers::internal_flow::flow_token_ok(&ctx, &headers) {
        return Ok(StatusCode::UNAUTHORIZED.into_response());
    }
    let stream = media_streams::Entity::find_by_id(q.stream_id)
        .filter(media_streams::Column::OrgId.eq(q.org_id))
        .filter(media_streams::Column::CaptureOn.eq(CaptureOn::Worker.wire()))
        .filter(media_streams::Column::DeletedAt.is_null())
        .one(&ctx.db)
        .await;
    let stream = match stream {
        Ok(Some(s)) => s,
        Ok(None) => return Ok(StatusCode::NOT_FOUND.into_response()),
        Err(e) => return db_error(e),
    };
    let Some(clock) = ClockSource::from_wire(&q.clock) else {
        return field("segment", err_codes::FIELD_INVALID);
    };
    match segments::accept_upload(&ctx, &stream, q.seq, q.started_ms, q.ended_ms, clock, &body)
        .await
    {
        Ok(_) => Ok(StatusCode::NO_CONTENT.into_response()),
        Err(UploadError::Invalid) => field("segment", err_codes::FIELD_INVALID),
        Err(UploadError::Unavailable) => Ok(StatusCode::SERVICE_UNAVAILABLE.into_response()),
    }
}

/// Upload cap of a model test clip (a few minutes of compressed audio).
const TEST_CLIP_MAX_BYTES: usize = 25 * 1024 * 1024;

#[derive(Debug, Deserialize, utoipa::IntoParams)]
#[into_params(parameter_in = Query)]
pub struct TestQuery {
    /// File name of the clip: its extension picks the demuxer when the
    /// content type does not.
    #[serde(default)]
    name: String,
    /// ISO 639-1 language code or `auto` (default `fr`).
    #[serde(default)]
    language: Option<String>,
}

/// `POST /api/v1/asr/models/{id}/test` — `can_write`: transcribes a dropped
/// audio clip (first 120 s) with a checked model (D167).
#[utoipa::path(
    post,
    path = "/api/v1/asr/models/{id}/test",
    tag = "asr",
    operation_id = "asr_models_test",
    summary = "Transcribe an audio clip with a model",
    description = "Transcribes the first 120 s of the uploaded audio clip (raw body, up to 25 MB; format from `Content-Type` or the `name` extension) with a checked speech-to-text model. Nothing is stored. Writer role required.",
    params(("id" = String, Path, format = "uuid", description = "Audio model UUID"), TestQuery),
    request_body(content = Vec<u8>, content_type = "application/octet-stream", description = "Audio file bytes"),
    responses(
        (status = 200, body = pnex_core::media_ingest::AsrTestResult),
        (status = 400, description = "Not a speech-to-text model (`asr-model-unsupported`), unreadable audio (`asr-test-audio-unsupported`) or invalid `language` (`field-invalid`)", body = crate::openapi::ErrorDetail),
        (status = 403, description = "Viewer role (`media-stream-write-forbidden`)", body = crate::openapi::ErrorDetail),
        (status = 404, description = "Unknown audio model (`asr-model-not-found`)", body = crate::openapi::ErrorDetail),
        (status = 409, description = "Model has not passed its check (`media-asr-model-invalid`)", body = crate::openapi::ErrorDetail),
        (status = 502, description = "Transcription failed (`asr-test-failed`)", body = crate::openapi::ErrorDetail),
        (status = 503, description = "ffmpeg missing on the server (`asr-test-failed`)", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn model_test(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(id): Path<Uuid>,
    Query(q): Query<TestQuery>,
    headers: axum::http::HeaderMap,
    body: axum::body::Bytes,
) -> Result<Response> {
    use crate::services::media_ingest::{asr, capture};
    use pnex_core::media_ingest::{is_valid_language, TEST_CLIP_MAX_SECS};
    use pnex_core::vision::ModelCheckStatus;
    require_write(&org)?;
    let row = match models::find(&ctx.db, org.org.id, id).await {
        Ok(row) => row,
        Err(e) => return model_error(e),
    };
    if row.task != asr::ASR_TASK {
        // VAD and diarization models have no transcript to show.
        return Err(detail(
            StatusCode::BAD_REQUEST,
            err_codes::ASR_MODEL_UNSUPPORTED,
            "Only a speech-to-text model can be tested with a clip.",
        ));
    }
    if row.check_status != ModelCheckStatus::Valid.wire() {
        return Err(detail(
            StatusCode::CONFLICT,
            err_codes::MEDIA_ASR_MODEL_INVALID,
            "The model has not passed its check.",
        ));
    }
    let language = q.language.unwrap_or_else(|| "fr".into());
    if !is_valid_language(&language) {
        return field("language", err_codes::FIELD_INVALID);
    }
    let unsupported = || {
        detail(
            StatusCode::BAD_REQUEST,
            err_codes::ASR_TEST_AUDIO_UNSUPPORTED,
            "The audio file cannot be read.",
        )
    };
    let content_type = headers
        .get(axum::http::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok());
    let format = capture::decoder::input_format(content_type, &q.name).ok_or_else(unsupported)?;
    let (wav, truncated) =
        match capture::decode_clip(body.to_vec(), format, 0, TEST_CLIP_MAX_SECS).await {
            Ok(v) => v,
            Err(capture::CaptureError::DecoderMissing) => {
                return Err(detail(
                    StatusCode::SERVICE_UNAVAILABLE,
                    err_codes::ASR_TEST_FAILED,
                    "ffmpeg is missing on the server.",
                ))
            }
            Err(_) => return Err(unsupported()),
        };
    match asr::test_clip(&ctx, &row, &wav, &language, truncated).await {
        Ok(result) => format::json(result),
        Err(e) => {
            tracing::warn!(model = %row.id, error = %e, "asr model test failed");
            Err(detail(
                StatusCode::BAD_GATEWAY,
                err_codes::ASR_TEST_FAILED,
                "The model could not transcribe the clip.",
            ))
        }
    }
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
    Ok(super::field_error(field, token))
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
            "This action is not available for this capture location.",
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
    /// Label filter `name` / `name:value` (D218), streams list only.
    label: Option<String>,
    /// Name contains (case-insensitive), streams list only.
    search: Option<String>,
}

// ───────────────────────────── Streams ─────────────────────────────

/// `GET /api/v1/media/streams` — every member (D14 envelope).
#[utoipa::path(
    get,
    path = "/api/v1/media/streams",
    tag = "media",
    operation_id = "media_streams_list",
    summary = "List the media streams of the organisation",
    description = "Paginated captured streams (radio, HLS, RTSP camera, replayed file…) with capture state and health. Secret values are never returned, only whether one is set and its vault reference.",
    params(("limit" = Option<i64>, Query, description = "Page size (default 10, max 100)"), ("offset" = Option<i64>, Query, description = "Page offset"), ("search" = Option<String>, Query, description = "Name contains (case-insensitive)"), ("label" = Option<String>, Query, description = "Label filter `name` or `name:value`")),
    responses(
        (status = 200, body = pnex_core::Paginated<pnex_core::media_ingest::MediaStream>),
        (status = 400, description = "Invalid `label` filter (`field-invalid`)", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn stream_list(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Query(q): Query<PageQuery>,
) -> Result<Response> {
    let page = pagination::PageParams::from(q.limit.as_deref(), q.offset.as_deref());
    let mut select = streams::select(org.org.id)
        .filter(crate::models::_entities::media_streams::Column::Pace.ne(streams::PACE_BATCH));
    if let Some(pat) = pagination::sql_search_pattern(q.search.as_deref()) {
        select = select.filter(pagination::sql_contains(
            (
                crate::models::_entities::media_streams::Entity,
                crate::models::_entities::media_streams::Column::Name,
            ),
            &pat,
        ));
    }
    match pagination::label_ids(
        &ctx.db,
        org.org.id,
        q.label.as_deref(),
        pnex_core::resources::KIND_MEDIA_STREAM,
    )
    .await
    {
        Ok(None) => {}
        Ok(Some(ids)) => {
            select = select.filter(
                crate::models::_entities::media_streams::Column::Id
                    .is_in(pagination::parse_ids::<uuid::Uuid>(&ids)),
            )
        }
        Err(resp) => return Ok(resp),
    }
    let (count, rows) = match pagination::sql_page(&ctx.db, select, page).await {
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

/// `GET /api/v1/media/capture-devices` — every member: the org's capture
/// boxes (agents that announced `media_capture`, lot 6b), for the
/// "capture location" picker. Names and ids only (R4).
#[utoipa::path(
    get,
    path = "/api/v1/media/capture-devices",
    tag = "media",
    operation_id = "media_capture_devices_list",
    summary = "List the capture boxes of the organisation",
    description = "Edge agents that announced the media capture capability, usable as `capture_on = device:<id>` for a stream. Names and ids only.",
    responses(
        (status = 200, body = pnex_core::Paginated<pnex_core::media_ingest::MediaCaptureDevice>)
    ),
    params(ListPage),
    security(("bearer" = [], "org" = []))
)]
async fn capture_device_list(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Query(q): Query<ListPage>,
) -> Result<Response> {
    match crate::services::media_ingest::boxes::list(&ctx.db, org.org.id).await {
        Ok(v) => paged("/api/v1/media/capture-devices", &q, v),
        Err(e) => db_error(e),
    }
}

/// Page parameters of the small org catalogues (capture boxes, ASR
/// profiles and models), paginated in memory.
#[derive(Debug, Deserialize, utoipa::IntoParams)]
#[into_params(parameter_in = Query)]
struct ListPage {
    /// Page size (default 10, max 100).
    #[param(value_type = Option<i64>)]
    limit: Option<String>,
    #[param(value_type = Option<i64>)]
    offset: Option<String>,
}

/// D14 envelope of an in-memory list.
// shortcut: the whole org list is loaded then sliced (tens of rows at most);
// push LIMIT/OFFSET into the services if a catalogue grows.
fn paged<T: serde::Serialize>(path: &str, q: &ListPage, all: Vec<T>) -> Result<Response> {
    let page = pagination::PageParams::from(q.limit.as_deref(), q.offset.as_deref());
    let count = all.len() as i64;
    let (skip, take) = page.slice(all.len());
    let rows: Vec<T> = all.into_iter().skip(skip).take(take).collect();
    format::json(pagination::envelope(path, &[], page, count, rows))
}

/// `GET /api/v1/media/streams/{id}` — every member.
#[utoipa::path(
    get,
    path = "/api/v1/media/streams/{id}",
    tag = "media",
    operation_id = "media_streams_get",
    summary = "Get a media stream",
    params(("id" = String, Path, format = "uuid", description = "Media stream UUID")),
    description = "One stream with its capture state and health; never the secret value.",
    responses(
        (status = 200, body = pnex_core::media_ingest::MediaStream),
        (status = 404, description = "Unknown stream or stream of another organisation (`media-stream-not-found`)", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
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
#[utoipa::path(
    post,
    path = "/api/v1/media/streams",
    tag = "media",
    operation_id = "media_streams_create",
    summary = "Create a media stream",
    description = "Registers a stream to capture and transcribe. An enabled stream starts being captured (network fetch, storage, ASR compute). The URL must not carry credentials: use `auth_secret` (a vault secret sent only to the URL's origin); binding a secret needs owner/admin. Writer role required. Limited number of streams per organisation.",
    request_body = pnex_core::media_ingest::MediaStreamInput,
    responses(
        (status = 201, body = pnex_core::media_ingest::MediaStream),
        (status = 400, description = "Per-field error `{\"<field>\": token}`, URL carrying a credential (`media-url-has-credentials`), unreachable stream (`media-stream-unreachable`) or capture location not supported (`media-capture-unsupported`)", body = crate::openapi::ErrorDetail),
        (status = 403, description = "Viewer role (`media-stream-write-forbidden`), or secret binding reserved to owner/admin", body = crate::openapi::ErrorDetail),
        (status = 404, description = "Referenced vault secret not found", body = crate::openapi::ErrorDetail),
        (status = 409, description = "Stream limit reached (`media-stream-limit`) or profile model not a valid audio model (`media-asr-model-invalid`)", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
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
#[utoipa::path(
    patch,
    path = "/api/v1/media/streams/{id}",
    tag = "media",
    operation_id = "media_streams_update",
    summary = "Update a media stream",
    description = "Absent fields keep their value; enabling/disabling starts or stops the capture. Moving a secret-holding stream to another origin, or changing its secret, needs owner/admin. Writer role required.",
    params(("id" = String, Path, format = "uuid", description = "Media stream UUID")),
    request_body = pnex_core::media_ingest::MediaStreamInput,
    responses(
        (status = 200, body = pnex_core::media_ingest::MediaStream),
        (status = 400, description = "Per-field error `{\"<field>\": token}`, URL carrying a credential (`media-url-has-credentials`), unreachable stream (`media-stream-unreachable`) or capture location not supported (`media-capture-unsupported`)", body = crate::openapi::ErrorDetail),
        (status = 403, description = "Viewer role (`media-stream-write-forbidden`), or secret change reserved to owner/admin", body = crate::openapi::ErrorDetail),
        (status = 404, description = "Unknown stream (`media-stream-not-found`) or referenced vault secret", body = crate::openapi::ErrorDetail),
        (status = 409, description = "Profile model not a valid audio model (`media-asr-model-invalid`)", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
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
#[utoipa::path(
    delete,
    path = "/api/v1/media/streams/{id}",
    tag = "media",
    operation_id = "media_streams_delete",
    summary = "Delete a media stream",
    description = "Stops the capture and deletes the stream with its segments and their stored audio. Transcripts already written to the telemetry store are not deleted. Writer role required.",
    params(("id" = String, Path, format = "uuid", description = "Media stream UUID")),
    responses(
        (status = 204, description = "Deleted"),
        (status = 403, description = "Viewer role (`media-stream-write-forbidden`)", body = crate::openapi::ErrorDetail),
        (status = 404, description = "Unknown stream or stream of another organisation (`media-stream-not-found`)", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
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

/// Longest wait for the first extract of a stream test.
const STREAM_TEST_LIMIT: std::time::Duration = std::time::Duration::from_secs(45);

/// `POST /api/v1/media/streams/{id}/test` — `can_write`: captures a first
/// extract of the saved stream (same filtered fetcher and confined decoder
/// as the capture, nothing stored, capture state untouched) and transcribes
/// it with the stream's profile (§7).
#[utoipa::path(
    post,
    path = "/api/v1/media/streams/{id}/test",
    tag = "media",
    operation_id = "media_streams_test",
    summary = "Test a media stream",
    description = "Captures a first extract of the saved stream (up to 45 s wait), then transcribes it with the stream's ASR profile. Nothing is stored and the capture state is untouched. Capture or transcription failures are reported in the 200 body (`capture_error`, `transcribe_error`). Not available for streams captured by a capture box. Writer role required.",
    params(("id" = String, Path, format = "uuid", description = "Media stream UUID")),
    responses(
        (status = 200, body = pnex_core::media_ingest::StreamTestResult),
        (status = 400, description = "Stream captured by a capture box (`media-capture-unsupported`)", body = crate::openapi::ErrorDetail),
        (status = 403, description = "Viewer role (`media-stream-write-forbidden`)", body = crate::openapi::ErrorDetail),
        (status = 404, description = "Unknown stream or stream of another organisation (`media-stream-not-found`)", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn stream_test(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(id): Path<Uuid>,
) -> Result<Response> {
    use crate::models::_entities::{asr_profiles, ml_models};
    use crate::services::media_ingest::{asr, capture};
    use pnex_core::media_ingest::StreamTestResult;
    use pnex_core::vision::ModelCheckStatus;
    require_write(&org)?;
    let stream = match streams::find(&ctx.db, org.org.id, id).await {
        Ok(s) => s,
        Err(e) => return stream_error(e),
    };
    // The test runs on the server: it cannot reach what a capture box sees.
    if stream.capture_on.starts_with("device:") {
        return stream_error(StreamError::CaptureUnsupported);
    }
    let seg = match capture::probe(&ctx, &stream, STREAM_TEST_LIMIT).await {
        Ok(seg) => seg,
        Err(e) => {
            return format::json(StreamTestResult {
                capture_error: Some(e.code().to_string()),
                ..Default::default()
            })
        }
    };
    let mut out = StreamTestResult {
        audio_ms: (seg.ended_at - seg.started_at).num_milliseconds().max(0) as u64,
        ..Default::default()
    };
    let profile = match stream.asr_profile_id {
        Some(p) => asr_profiles::Entity::find_by_id(p)
            .filter(asr_profiles::Column::OrgId.eq(org.org.id))
            .one(&ctx.db)
            .await
            .ok()
            .flatten(),
        None => None,
    };
    let Some(profile) = profile else {
        out.transcribe_error = Some("no-profile".into());
        return format::json(out);
    };
    let model = ml_models::Entity::find_by_id(profile.asr_model_id)
        .filter(ml_models::Column::OrgId.eq(org.org.id))
        .one(&ctx.db)
        .await
        .ok()
        .flatten()
        .filter(|m| m.check_status == ModelCheckStatus::Valid.wire());
    let Some(model) = model else {
        out.transcribe_error = Some("model-invalid".into());
        return format::json(out);
    };
    match asr::test_clip(&ctx, &model, &seg.wav, &profile.language, false).await {
        Ok(r) => out.text = Some(r.text),
        Err(e) => {
            tracing::warn!(stream = %stream.slug, error = %e, "stream test transcription failed");
            out.transcribe_error = Some("asr-failed".into());
        }
    }
    format::json(out)
}

/// `GET /api/v1/media/streams/{id}/segments` — every member, newest
/// first, optional `state` filter (D14 envelope).
#[utoipa::path(
    get,
    path = "/api/v1/media/streams/{id}/segments",
    tag = "media",
    operation_id = "media_streams_segments_list",
    summary = "List the captured segments of a stream",
    description = "Paginated audio segments, newest first, with their transcription state.",
    params(("id" = String, Path, format = "uuid", description = "Media stream UUID"), ("limit" = Option<i64>, Query, description = "Page size (default 10, max 100)"), ("offset" = Option<i64>, Query, description = "Page offset"), ("state" = Option<String>, Query, description = "Segment state filter (`captured`, `queued`, `transcribing`, `transcribed`, `failed`, `skipped_silence`, `skipped_backlog`)")),
    responses(
        (status = 200, body = pnex_core::Paginated<pnex_core::media_ingest::MediaSegment>),
        (status = 400, description = "Unknown `state` (`field-invalid`)", body = crate::openapi::ErrorDetail),
        (status = 404, description = "Unknown stream or stream of another organisation (`media-stream-not-found`)", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
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
#[utoipa::path(
    post,
    path = "/api/v1/media/streams/{id}/segments/retry",
    tag = "media",
    operation_id = "media_streams_segments_retry",
    summary = "Retry the failed segments of a stream",
    description = "Re-queues for transcription every failed segment whose audio is still stored (consumes ASR compute). Writer role required.",
    params(("id" = String, Path, format = "uuid", description = "Media stream UUID")),
    responses(
        (status = 200, body = MediaSegmentsRequeued),
        (status = 403, description = "Viewer role (`media-stream-write-forbidden`)", body = crate::openapi::ErrorDetail),
        (status = 404, description = "Unknown stream or stream of another organisation (`media-stream-not-found`)", body = crate::openapi::ErrorDetail),
        (status = 409, description = "Stream has no ASR profile (`media-asr-model-invalid`)", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
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
        crate::services::media_ingest::segments::enqueue(&ctx, seg, false).await;
    }
    format::json(MediaSegmentsRequeued {
        requeued: failed.len(),
    })
}

/// Number of failed segments re-queued for transcription.
#[derive(serde::Serialize, utoipa::ToSchema)]
struct MediaSegmentsRequeued {
    requeued: usize,
}

fn segment_view(s: &media_segments::Model) -> MediaSegment {
    MediaSegment {
        id: s.id.to_string(),
        seq: s.seq,
        started_at: s.started_at.to_rfc3339(),
        ended_at: s.ended_at.to_rfc3339(),
        clock_source: pnex_core::media_ingest::ClockSource::from_wire(&s.clock_source)
            .unwrap_or(pnex_core::media_ingest::ClockSource::Host),
        state: SegmentState::from_wire(&s.state).unwrap_or(SegmentState::Failed),
        size_bytes: s.size_bytes,
        asr_model_id: s.asr_model_id.map(|v| v.to_string()),
        asr_ms: s.asr_ms,
        error: s.error.clone(),
    }
}

// ───────────────────────────── Profiles ─────────────────────────────

/// `GET /api/v1/asr/profiles` — every member.
#[utoipa::path(
    get,
    path = "/api/v1/asr/profiles",
    tag = "asr",
    operation_id = "asr_profiles_list",
    summary = "List the ASR profiles of the organisation",
    description = "Paginated transcription profiles (`limit` default 10, max 100; `offset`): speech-to-text model, optional VAD and diarization models, language, beam.",
    params(ListPage),
    responses(
        (status = 200, body = pnex_core::Paginated<pnex_core::media_ingest::AsrProfile>)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn profile_list(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Query(q): Query<ListPage>,
) -> Result<Response> {
    match profiles::list(&ctx.db, org.org.id).await {
        Ok(rows) => paged(
            "/api/v1/asr/profiles",
            &q,
            rows.iter().map(profiles::view).collect(),
        ),
        Err(e) => db_error(e),
    }
}

/// `GET /api/v1/asr/profiles/{id}` — every member.
#[utoipa::path(
    get,
    path = "/api/v1/asr/profiles/{id}",
    tag = "asr",
    operation_id = "asr_profiles_get",
    summary = "Get an ASR profile",
    description = "One transcription profile.",
    params(("id" = String, Path, format = "uuid", description = "ASR profile UUID")),
    responses(
        (status = 200, body = pnex_core::media_ingest::AsrProfile),
        (status = 404, description = "Unknown ASR profile (`asr-profile-not-found`)", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
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
#[utoipa::path(
    post,
    path = "/api/v1/asr/profiles",
    tag = "asr",
    operation_id = "asr_profiles_create",
    summary = "Create an ASR profile",
    description = "Creates a transcription profile; diarization needs both its segmentation and embedding models. Writer role required.",
    request_body = pnex_core::media_ingest::AsrProfileInput,
    responses(
        (status = 201, body = pnex_core::media_ingest::AsrProfile),
        (status = 400, description = "`field-invalid` with machine tokens in `errors.fields`", body = crate::openapi::ErrorDetail),
        (status = 403, description = "Viewer role (`media-stream-write-forbidden`)", body = crate::openapi::ErrorDetail),
        (status = 409, description = "Name already used (`asr-profile-name-taken`)", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
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
#[utoipa::path(
    patch,
    path = "/api/v1/asr/profiles/{id}",
    tag = "asr",
    operation_id = "asr_profiles_update",
    summary = "Update an ASR profile",
    description = "Absent fields keep their value; an empty string clears an optional model. Applies to the next transcribed segments of the streams using it. Writer role required.",
    params(("id" = String, Path, format = "uuid", description = "ASR profile UUID")),
    request_body = pnex_core::media_ingest::AsrProfileInput,
    responses(
        (status = 200, body = pnex_core::media_ingest::AsrProfile),
        (status = 400, description = "`field-invalid` with machine tokens in `errors.fields`", body = crate::openapi::ErrorDetail),
        (status = 403, description = "Viewer role (`media-stream-write-forbidden`)", body = crate::openapi::ErrorDetail),
        (status = 404, description = "Unknown ASR profile (`asr-profile-not-found`)", body = crate::openapi::ErrorDetail),
        (status = 409, description = "Name already used (`asr-profile-name-taken`)", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
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
#[utoipa::path(
    delete,
    path = "/api/v1/asr/profiles/{id}",
    tag = "asr",
    operation_id = "asr_profiles_delete",
    summary = "Delete an ASR profile",
    description = "Deletes the profile; streams using it stop being transcribed until another profile is set. Writer role required.",
    params(("id" = String, Path, format = "uuid", description = "ASR profile UUID")),
    responses(
        (status = 204, description = "Deleted"),
        (status = 403, description = "Viewer role (`media-stream-write-forbidden`)", body = crate::openapi::ErrorDetail),
        (status = 404, description = "Unknown ASR profile (`asr-profile-not-found`)", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
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
        ModelError::InUse { profiles } => Err(super::coded_error(
            StatusCode::CONFLICT,
            err_codes::ASR_MODEL_IN_USE,
            format!(
                "The model is used by ASR profiles: {}. Delete with force=true to remove them too.",
                profiles.join(", ")
            ),
            Some(serde_json::json!({ "profiles": profiles.join(", ") })),
        )),
        ModelError::Unsupported => Err(detail(
            StatusCode::BAD_REQUEST,
            err_codes::ASR_MODEL_UNSUPPORTED,
            "The file is not a supported audio model.",
        )),
        ModelError::RuntimeUnavailable { family } => Err(super::coded_error(
            StatusCode::UNPROCESSABLE_ENTITY,
            err_codes::ASR_RUNTIME_UNAVAILABLE,
            format!("No runtime of this server can run models of family {family}."),
            Some(serde_json::json!({ "family": family })),
        )),
        ModelError::Db(e) => db_error(e),
    }
}

/// One model with its per-carrier checks.
async fn model_view(
    ctx: &AppContext,
    row: &crate::models::_entities::ml_models::Model,
    status: StatusCode,
) -> Result<Response> {
    match models::views(&ctx.db, std::slice::from_ref(row)).await {
        Ok(mut v) => Ok((status, format::json(v.remove(0))).into_response()),
        Err(e) => db_error(e),
    }
}

/// `GET /api/v1/asr/models` — every member.
#[utoipa::path(
    get,
    path = "/api/v1/asr/models",
    tag = "asr",
    operation_id = "asr_models_list",
    summary = "List the audio models of the organisation",
    description = "Paginated audio models of the registry (`limit` default 10, max 100; `offset`): speech-to-text, VAD, diarization, speaker embedding, with their check results on the server and on transcription workers.",
    params(ListPage),
    responses(
        (status = 200, body = pnex_core::Paginated<pnex_core::media_ingest::AsrModel>)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn model_list(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Query(q): Query<ListPage>,
) -> Result<Response> {
    match models::list(&ctx.db, org.org.id).await {
        Ok(rows) => match models::views(&ctx.db, &rows).await {
            Ok(v) => paged("/api/v1/asr/models", &q, v),
            Err(e) => db_error(e),
        },
        Err(e) => db_error(e),
    }
}

/// `GET /api/v1/asr/models/{id}` — every member.
#[utoipa::path(
    get,
    path = "/api/v1/asr/models/{id}",
    tag = "asr",
    operation_id = "asr_models_get",
    summary = "Get an audio model",
    description = "One audio model with its checks.",
    params(("id" = String, Path, format = "uuid", description = "Audio model UUID")),
    responses(
        (status = 200, body = pnex_core::media_ingest::AsrModel),
        (status = 404, description = "Unknown audio model (`asr-model-not-found`)", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn model_get(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(id): Path<Uuid>,
) -> Result<Response> {
    match models::find(&ctx.db, org.org.id, id).await {
        Ok(row) => model_view(&ctx, &row, StatusCode::OK).await,
        Err(e) => model_error(e),
    }
}

/// `POST /api/v1/asr/models` — `can_write`. The files are read and the
/// model checked before the answer (family read, never typed).
#[utoipa::path(
    post,
    path = "/api/v1/asr/models",
    tag = "asr",
    operation_id = "asr_models_create",
    summary = "Register an audio model",
    description = "Registers a `model` media asset (archive or GGML file) as an audio model. The files are read and the model is loaded and checked on the server before the answer (family and task are read from the files, never typed); this can take a while. Writer role required.",
    request_body = pnex_core::media_ingest::AsrModelInput,
    responses(
        (status = 201, body = pnex_core::media_ingest::AsrModel),
        (status = 400, description = "Per-field error `{\"<field>\": token}`, or not a supported audio model (`asr-model-unsupported`)", body = crate::openapi::ErrorDetail),
        (status = 403, description = "Viewer role (`media-stream-write-forbidden`)", body = crate::openapi::ErrorDetail),
        (status = 409, description = "Name already used (`asr-model-name-taken`)", body = crate::openapi::ErrorDetail),
        (status = 422, description = "No runtime of this server runs this model family (`asr-runtime-unavailable`, `errors.args.family`)", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn model_create(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Json(input): Json<AsrModelInput>,
) -> Result<Response> {
    require_write(&org)?;
    match models::create(&ctx, org.org.id, &input).await {
        Ok(row) => model_view(&ctx, &row, StatusCode::CREATED).await,
        Err(e) => model_error(e),
    }
}

/// `PATCH /api/v1/asr/models/{id}` — `can_write` (name, description,
/// license; the file never changes).
#[utoipa::path(
    patch,
    path = "/api/v1/asr/models/{id}",
    tag = "asr",
    operation_id = "asr_models_update",
    summary = "Update an audio model",
    description = "Name, description and license only; the model file never changes. Writer role required.",
    params(("id" = String, Path, format = "uuid", description = "Audio model UUID")),
    request_body = pnex_core::media_ingest::AsrModelInput,
    responses(
        (status = 200, body = pnex_core::media_ingest::AsrModel),
        (status = 400, description = "`field-invalid` with machine tokens in `errors.fields`", body = crate::openapi::ErrorDetail),
        (status = 403, description = "Viewer role (`media-stream-write-forbidden`)", body = crate::openapi::ErrorDetail),
        (status = 404, description = "Unknown audio model (`asr-model-not-found`)", body = crate::openapi::ErrorDetail),
        (status = 409, description = "Name already used (`asr-model-name-taken`)", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn model_update(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(id): Path<Uuid>,
    Json(input): Json<AsrModelInput>,
) -> Result<Response> {
    require_write(&org)?;
    match models::update(&ctx, org.org.id, id, &input).await {
        Ok(row) => model_view(&ctx, &row, StatusCode::OK).await,
        Err(e) => model_error(e),
    }
}

#[derive(Debug, Deserialize, utoipa::IntoParams)]
#[into_params(parameter_in = Query)]
struct DeleteModelQuery {
    /// Also delete the ASR profiles that use the model (default false).
    #[serde(default)]
    force: bool,
}

/// `DELETE /api/v1/asr/models/{id}` — `can_write`.
#[utoipa::path(
    delete,
    path = "/api/v1/asr/models/{id}",
    tag = "asr",
    operation_id = "asr_models_delete",
    summary = "Delete an audio model",
    description = "Removes the model from the registry; the model's media asset is kept. While ASR profiles use the model, answers 409 `asr-model-in-use` (`errors.args.profiles` = their names) unless `force=true`: then the profiles whose speech-to-text model it is are deleted too (their streams stop being transcribed until another profile is set) and it is cleared from the VAD/diarization slots of the others. Writer role required.",
    params(("id" = String, Path, format = "uuid", description = "Audio model UUID"), DeleteModelQuery),
    responses(
        (status = 204, description = "Deleted"),
        (status = 403, description = "Viewer role (`media-stream-write-forbidden`)", body = crate::openapi::ErrorDetail),
        (status = 404, description = "Unknown audio model (`asr-model-not-found`)", body = crate::openapi::ErrorDetail),
        (status = 409, description = "Profiles use the model (`asr-model-in-use`, `errors.args.profiles`)", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn model_delete(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(id): Path<Uuid>,
    Query(q): Query<DeleteModelQuery>,
) -> Result<Response> {
    require_write(&org)?;
    match models::delete(&ctx, org.org.id, id, q.force).await {
        Ok(()) => Ok(StatusCode::NO_CONTENT.into_response()),
        Err(e) => model_error(e),
    }
}

/// `POST /api/v1/asr/models/{id}/check` — `can_write`: reload the model and
/// transcribe the reference sample on this server.
#[utoipa::path(
    post,
    path = "/api/v1/asr/models/{id}/check",
    tag = "asr",
    operation_id = "asr_models_check",
    summary = "Re-check an audio model",
    description = "Reloads the model and transcribes the reference sample on this server, updating its check (load time, real-time factor, word error rate). Synchronous and compute-heavy. Writer role required.",
    params(("id" = String, Path, format = "uuid", description = "Audio model UUID")),
    responses(
        (status = 200, body = pnex_core::media_ingest::AsrModel),
        (status = 400, description = "Not a supported audio model (`asr-model-unsupported`)", body = crate::openapi::ErrorDetail),
        (status = 403, description = "Viewer role (`media-stream-write-forbidden`)", body = crate::openapi::ErrorDetail),
        (status = 404, description = "Unknown audio model (`asr-model-not-found`)", body = crate::openapi::ErrorDetail),
        (status = 422, description = "No runtime for this model family (`asr-runtime-unavailable`)", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
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
        Ok(row) => model_view(&ctx, &row, StatusCode::OK).await,
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
#[utoipa::path(
    get,
    path = "/api/v1/media/transcripts",
    tag = "media",
    operation_id = "media_transcripts_search",
    summary = "Search the transcripts of the organisation's streams",
    description = "Paginated transcribed segments, read from the telemetry store, optionally filtered by streams, text and time range.",
    params(("stream" = Option<String>, Query, description = "Comma-separated stream slugs; empty = every stream of the organisation"), ("q" = Option<String>, Query, description = "Text to search (max length enforced)"), ("from" = Option<String>, Query, format = DateTime, description = "Lower time bound, RFC 3339"), ("to" = Option<String>, Query, format = DateTime, description = "Upper time bound, RFC 3339"), ("limit" = Option<i64>, Query, description = "Page size (default 10, max 100)"), ("offset" = Option<i64>, Query, description = "Page offset")),
    responses(
        (status = 200, body = pnex_core::Paginated<pnex_core::media_ingest::TranscriptRecord>),
        (status = 400, description = "`field-invalid`: `q` `max_length:N`, `from`/`to` `invalid`", body = crate::openapi::ErrorDetail),
        (status = 404, description = "A listed stream slug is unknown (`media-stream-not-found`)", body = crate::openapi::ErrorDetail),
        (status = 502, description = "Telemetry store unavailable (`media-transcripts-unavailable`)", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
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

/// `GET /api/v1/media/metadata?stream=<slug>` — every member: in-band
/// metadata events of one stream of the org (D170), newest first. The slug
/// is resolved to a stream row of the org; the O2 stream name is built
/// server side.
#[utoipa::path(
    get,
    path = "/api/v1/media/metadata",
    tag = "media",
    operation_id = "media_metadata_list",
    summary = "List the in-band metadata events of a stream",
    description = "Paginated in-band metadata (e.g. ICY track titles) of one stream, newest first, read from the telemetry store.",
    params(("stream" = String, Query, description = "Stream slug (required)"), ("from" = Option<String>, Query, format = DateTime, description = "Lower time bound, RFC 3339"), ("to" = Option<String>, Query, format = DateTime, description = "Upper time bound, RFC 3339"), ("limit" = Option<i64>, Query, description = "Page size (default 10, max 100)"), ("offset" = Option<i64>, Query, description = "Page offset")),
    responses(
        (status = 200, body = pnex_core::Paginated<pnex_core::media_ingest::MetadataRecord>),
        (status = 400, description = "`field-invalid`: `stream` `required`, `from`/`to` `invalid`", body = crate::openapi::ErrorDetail),
        (status = 404, description = "Unknown stream slug (`media-stream-not-found`)", body = crate::openapi::ErrorDetail),
        (status = 502, description = "Telemetry store unavailable (`media-transcripts-unavailable`)", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn metadata_list(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Query(q): Query<TranscriptQuery>,
) -> Result<Response> {
    let page = pagination::PageParams::from(q.limit.as_deref(), q.offset.as_deref());
    let Some(slug) = q.stream.as_deref().map(str::trim).filter(|s| !s.is_empty()) else {
        return field("stream", err_codes::FIELD_REQUIRED);
    };
    let (from_us, to_us) = match (
        parse_time("from", q.from.as_deref()),
        parse_time("to", q.to.as_deref()),
    ) {
        (Ok(f), Ok(t)) => (f, t),
        (Err(f), _) | (_, Err(f)) => return field(f, err_codes::FIELD_INVALID),
    };
    let row = match streams::find_by_slug(&ctx.db, org.org.id, slug).await {
        Ok(Some(row)) => row,
        Ok(None) => return stream_error(StreamError::NotFound),
        Err(e) => return db_error(e),
    };
    match crate::services::media_ingest::metadata::list(
        &ctx,
        org.org.id,
        &row.slug,
        from_us,
        to_us,
        page.offset,
        page.limit,
    )
    .await
    {
        Ok((count, results)) => format::json(pagination::envelope(
            "/api/v1/media/metadata",
            &[("stream".to_string(), row.slug.clone())],
            page,
            count,
            results,
        )),
        Err(e) => {
            tracing::warn!(error = %e, "metadata read failed");
            Err(detail(
                StatusCode::BAD_GATEWAY,
                err_codes::MEDIA_TRANSCRIPTS_UNAVAILABLE,
                "In-band metadata is unavailable on OpenObserve.",
            ))
        }
    }
}
