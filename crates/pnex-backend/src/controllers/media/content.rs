use super::*;
use axum::http::{header, HeaderMap};

// ───────────────── GET /media/{id}/…/content (bytes) ─────────────────

/// `GET /api/v1/media/{id}/versions/{n}`: one version's metadata.
#[utoipa::path(
    get,
    path = "/api/v1/media/{id}/versions/{n}",
    tag = "media",
    operation_id = "media_versions_get",
    summary = "Get one version of a media asset",
    params(("id" = String, Path, format = "uuid", description = "Media asset UUID"), ("n" = i64, Path, description = "Version number (1-based)")),
    description = "Metadata of version n (filename, content type, size, sha256, note, whether current).",
    responses(
        (status = 200, body = MediaVersionDto),
        (status = 404, description = "Unknown asset or version", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
pub(super) async fn version_of(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(path): Path<VersionPath>,
) -> Result<Response> {
    let Some(asset) = find_asset(&ctx.db, &org, path.id).await? else {
        return Err(Error::NotFound);
    };
    let Some(version) = find_version(&ctx.db, asset.id, path.n).await? else {
        return Err(Error::NotFound);
    };
    Ok(format::json(version_dto(&version, asset.current_version_id)).into_response())
}

/// Strong validator of a version's bytes: a version is immutable (append-only
/// versioning), so its id identifies the content for good.
fn version_etag(version: &media_versions::Model) -> String {
    format!("\"{}\"", version.id)
}

/// True when the `If-None-Match` header already names this version.
fn not_modified(headers: &HeaderMap, etag: &str) -> bool {
    headers
        .get(header::IF_NONE_MATCH)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| {
            v.split(',')
                .any(|tag| tag.trim() == etag || tag.trim() == "*")
        })
}

/// Bytes of a version, served inline (builds.rs::download school).
/// Revalidated by ETag: a client holding the version (browser media cache)
/// gets a bodiless 304, checked AFTER the org scoping of the caller.
async fn content_response(
    ctx: &AppContext,
    _asset: &media_assets::Model,
    version: &media_versions::Model,
    request_headers: &HeaderMap,
) -> Result<Response> {
    let etag = version_etag(version);
    // `private`: per-user content, never in a shared cache; `no-cache`: always
    // revalidated, so a new version or a lost access shows up immediately.
    let cache_headers = [
        (header::ETAG, etag.clone()),
        (header::CACHE_CONTROL, "private, no-cache".to_string()),
    ];
    if not_modified(request_headers, &etag) {
        return Ok((StatusCode::NOT_MODIFIED, cache_headers).into_response());
    }
    let settings = MediaSettings::from_config(&ctx.config);
    let store = settings.store().map_err(|_| Error::InternalServerError)?;
    let bytes = store
        .get(&version.storage_key)
        .await
        .map_err(|_| Error::NotFound)?;
    // Served type re-derived from the allowlist (rows written before SEC-5
    // may hold a client-declared type).
    let headers =
        crate::services::media::user_content_headers(&version.content_type, &version.filename);
    Ok((StatusCode::OK, cache_headers, headers, bytes).into_response())
}

/// `GET /api/v1/media/{id}/content`: bytes of the current version.
#[utoipa::path(
    get,
    path = "/api/v1/media/{id}/content",
    tag = "media",
    operation_id = "media_content",
    summary = "Download the current version's file",
    description = "Raw bytes of the current version, served with an allowlisted content type and an `ETag` (the version id). Send `If-None-Match` to get a bodiless 304 when unchanged.",
    params(("id" = String, Path, format = "uuid", description = "Media asset UUID")),
    responses(
        (status = 200, content_type = "application/octet-stream", body = Vec<u8>, description = "File bytes (actual Content-Type depends on the file)"),
        (status = 304, description = "Not modified (`If-None-Match` matched)"),
        (status = 404, description = "Unknown asset, no version, or stored file missing", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
pub(super) async fn content(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(id): Path<Uuid>,
    request_headers: HeaderMap,
) -> Result<Response> {
    let Some(asset) = find_asset(&ctx.db, &org, id).await? else {
        return Err(Error::NotFound);
    };
    let Some(current_id) = asset.current_version_id else {
        return Err(Error::NotFound);
    };
    let Some(version) = media_versions::Entity::find_by_id(current_id)
        .one(&ctx.db)
        .await
        .map_err(|_| Error::InternalServerError)?
    else {
        return Err(Error::NotFound);
    };
    content_response(&ctx, &asset, &version, &request_headers).await
}

/// `GET /api/v1/media/{id}/versions/{n}/content`: bytes of version n.
#[utoipa::path(
    get,
    path = "/api/v1/media/{id}/versions/{n}/content",
    tag = "media",
    operation_id = "media_versions_content",
    summary = "Download one version's file",
    description = "Raw bytes of version n, with `ETag` revalidation (`If-None-Match` gives a bodiless 304).",
    params(("id" = String, Path, format = "uuid", description = "Media asset UUID"), ("n" = i64, Path, description = "Version number (1-based)")),
    responses(
        (status = 200, content_type = "application/octet-stream", body = Vec<u8>, description = "File bytes (actual Content-Type depends on the file)"),
        (status = 304, description = "Not modified (`If-None-Match` matched)"),
        (status = 404, description = "Unknown asset or version, or stored file missing", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
pub(super) async fn version_content(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(path): Path<VersionPath>,
    request_headers: HeaderMap,
) -> Result<Response> {
    let Some(asset) = find_asset(&ctx.db, &org, path.id).await? else {
        return Err(Error::NotFound);
    };
    let Some(version) = find_version(&ctx.db, asset.id, path.n).await? else {
        return Err(Error::NotFound);
    };
    content_response(&ctx, &asset, &version, &request_headers).await
}
