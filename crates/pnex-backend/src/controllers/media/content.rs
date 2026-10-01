use super::*;

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

/// Octets d'une version — réponse inline (école builds.rs::download).
async fn content_response(
    ctx: &AppContext,
    _asset: &media_assets::Model,
    version: &media_versions::Model,
) -> Result<Response> {
    let settings = MediaSettings::from_config(&ctx.config);
    let store = settings.store().map_err(|_| Error::InternalServerError)?;
    let bytes = store
        .get(&version.storage_key)
        .await
        .map_err(|_| Error::NotFound)?;
    let filename = pnex_firmware_builder::sanitize_segment(&version.filename);
    Ok((
        StatusCode::OK,
        [
            ("content-type", version.content_type.clone()),
            (
                "content-disposition",
                format!("inline; filename=\"{filename}\""),
            ),
        ],
        bytes,
    )
        .into_response())
}

/// `GET /api/v1/media/{id}/content` — octets de la version courante.
pub(super) async fn content(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(id): Path<Uuid>,
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
    content_response(&ctx, &asset, &version).await
}

/// `GET /api/v1/media/{id}/versions/{n}/content` — octets de la version n.
pub(super) async fn version_content(
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
    content_response(&ctx, &asset, &version).await
}
