use super::*;
use axum::http::{header, HeaderMap};

// ───────────────── GET /media/{id}/…/content (octets) ─────────────────

/// `GET /api/v1/media/{id}/versions/{n}` — détail d'une version.
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

/// Octets d'une version — réponse inline (école builds.rs::download).
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

/// `GET /api/v1/media/{id}/content` — octets de la version courante.
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

/// `GET /api/v1/media/{id}/versions/{n}/content` — octets de la version n.
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
