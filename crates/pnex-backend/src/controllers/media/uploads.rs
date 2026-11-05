use super::*;

use axum::body::Body;
use pnex_core::media_upload::{MediaUploadComplete, MediaUploadInput};

use crate::services::media_uploads::{self, UploadError};

// ───────────────────── Uploads in parts (data-pipelines.md §9.9) ─────────────────────

fn upload_error(e: UploadError) -> Result<Response> {
    let (status, text) = match &e {
        UploadError::Invalid { field, token } => return Ok(field_status(field, token)),
        UploadError::Storage(err) => {
            tracing::warn!("media upload: {err}");
            return Err(Error::InternalServerError);
        }
        UploadError::Db(err) => {
            tracing::warn!("media upload: {err}");
            return Err(Error::InternalServerError);
        }
        UploadError::NotFound => (StatusCode::NOT_FOUND, "No such upload in this org."),
        UploadError::Closed => (StatusCode::CONFLICT, "The upload no longer takes parts."),
        UploadError::PartTooLarge => (
            StatusCode::PAYLOAD_TOO_LARGE,
            "A part is over the size allowed for one request.",
        ),
        UploadError::Interrupted => (StatusCode::BAD_REQUEST, "The part stopped before its end."),
        UploadError::PartsMissing { .. } => {
            (StatusCode::CONFLICT, "Parts of the file are missing.")
        }
        UploadError::TooMany => (
            StatusCode::CONFLICT,
            "Too many uploads are in progress in this org.",
        ),
        UploadError::Lost => (StatusCode::CONFLICT, "Joining the file failed."),
        UploadError::FileTooLarge { .. } => (
            StatusCode::PAYLOAD_TOO_LARGE,
            "File exceeds the upload size limit (PNEX_MEDIA_MAX_BYTES)",
        ),
        UploadError::Format => return Err(format_unsupported()),
    };
    Err(crate::controllers::coded_error(
        status,
        e.code(),
        text,
        e.args(),
    ))
}

/// `POST /api/v1/media/uploads`
#[utoipa::path(
    post,
    path = "/api/v1/media/uploads",
    tag = "media",
    operation_id = "media_uploads_start",
    summary = "Open the upload of a media file in parts",
    description = "For a file over what a request may carry behind a proxy (Cloudflare: 100 MB): a new asset, or a new version of `asset_id`. Send the parts with `PUT .../parts/{part}`, close with `POST .../complete`, then read the upload until it is `done` (its `asset_id` and `version`) or `failed` (its `code`). The format is judged on the whole file, as for a single-request upload. Owner, admin or member.",
    request_body = pnex_core::media_upload::MediaUploadInput,
    responses(
        (status = 201, body = pnex_core::media_upload::MediaUpload),
        (status = 400, description = "`field-invalid` (`filename`: `required`; `kind`, `asset_id`: `invalid`)", body = crate::openapi::ErrorDetail),
        (status = 403, description = "`access-denied`", body = crate::openapi::ErrorDetail),
        (status = 409, description = "`media-upload-too-many` (args `max`)", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
pub(super) async fn upload_start(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Json(input): Json<MediaUploadInput>,
) -> Result<Response> {
    let kind = input
        .kind
        .as_deref()
        .map(str::trim)
        .filter(|k| !k.is_empty());
    if kind.is_some_and(|k| MediaKind::parse(k).is_none()) {
        return Ok(field_status("kind", pnex_core::err_codes::FIELD_INVALID));
    }
    match media_uploads::start(&ctx.db, org.org.id, &input).await {
        Ok(u) => Ok((StatusCode::CREATED, Json(u)).into_response()),
        Err(e) => upload_error(e),
    }
}

/// `GET /api/v1/media/uploads/{upload}`
#[utoipa::path(
    get,
    path = "/api/v1/media/uploads/{upload}",
    tag = "media",
    operation_id = "media_uploads_get",
    summary = "State of a media upload",
    description = "Parts received, then the join: `done` with its `asset_id` and `version`, or `failed` with its `code` and `args`. Owner, admin, member or viewer.",
    params(("upload" = String, Path, format = "uuid", description = "Upload id (UUID)")),
    responses(
        (status = 200, body = pnex_core::media_upload::MediaUpload),
        (status = 403, description = "`access-denied`", body = crate::openapi::ErrorDetail),
        (status = 404, description = "`media-upload-not-found`", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
pub(super) async fn upload_get(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(upload): Path<Uuid>,
) -> Result<Response> {
    match media_uploads::get(&ctx.db, org.org.id, upload).await {
        Ok(u) => format::json(u),
        Err(e) => upload_error(e),
    }
}

/// `PUT /api/v1/media/uploads/{upload}/parts/{part}`
#[utoipa::path(
    put,
    path = "/api/v1/media/uploads/{upload}/parts/{part}",
    tag = "media",
    operation_id = "media_uploads_part_put",
    summary = "Store one part of a media upload",
    description = "The bytes of the file, cut anywhere, 64 MiB at most. Parts are numbered from 1 and may come in any order; sending one again replaces it. Owner, admin or member.",
    params(
        ("upload" = String, Path, format = "uuid", description = "Upload id (UUID)"),
        ("part" = i32, Path, description = "Part number, from 1 to 1000")
    ),
    request_body(content = String, content_type = "application/octet-stream"),
    responses(
        (status = 200, body = pnex_core::media_upload::MediaUpload),
        (status = 400, description = "`field-invalid` (`part`), `media-upload-interrupted`", body = crate::openapi::ErrorDetail),
        (status = 403, description = "`access-denied`", body = crate::openapi::ErrorDetail),
        (status = 404, description = "`media-upload-not-found`", body = crate::openapi::ErrorDetail),
        (status = 409, description = "`media-upload-closed`", body = crate::openapi::ErrorDetail),
        (status = 413, description = "`media-upload-part-too-large` (args `max`), `media-file-too-large` (args `max_bytes`)", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
pub(super) async fn upload_part(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path((upload, part)): Path<(Uuid, i32)>,
    body: Body,
) -> Result<Response> {
    let settings = MediaSettings::from_config(&ctx.config);
    let store = settings.storage.store().map_err(|e| {
        tracing::warn!("media upload: {e}");
        Error::InternalServerError
    })?;
    let stream = body.into_data_stream();
    let max = settings.max_bytes as u64;
    match media_uploads::put_part(&ctx.db, &store, max, org.org.id, upload, part, stream).await {
        Ok(u) => format::json(u),
        Err(e) => upload_error(e),
    }
}

/// `POST /api/v1/media/uploads/{upload}/complete`
#[utoipa::path(
    post,
    path = "/api/v1/media/uploads/{upload}/complete",
    tag = "media",
    operation_id = "media_uploads_complete",
    summary = "Close a media upload and join the file",
    description = "Says how many parts make the file; refused if any is missing. A server then joins them in the background: read the upload until it is `done` or `failed` (`media-format-unsupported` for a refused format). Owner, admin or member.",
    params(("upload" = String, Path, format = "uuid", description = "Upload id (UUID)")),
    request_body = pnex_core::media_upload::MediaUploadComplete,
    responses(
        (status = 202, body = pnex_core::media_upload::MediaUpload),
        (status = 403, description = "`access-denied`", body = crate::openapi::ErrorDetail),
        (status = 404, description = "`media-upload-not-found`", body = crate::openapi::ErrorDetail),
        (status = 409, description = "`media-upload-closed`, `media-upload-parts-missing` (args `missing`, `count`)", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
pub(super) async fn upload_complete(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(upload): Path<Uuid>,
    Json(input): Json<MediaUploadComplete>,
) -> Result<Response> {
    match media_uploads::complete(&ctx.db, org.org.id, upload, input.parts).await {
        Ok(u) => Ok((StatusCode::ACCEPTED, Json(u)).into_response()),
        Err(e) => upload_error(e),
    }
}
