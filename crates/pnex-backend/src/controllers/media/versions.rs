use super::*;

// ─────────────────────── POST /media/{id}/versions ───────────────────────

#[derive(Deserialize, utoipa::IntoParams)]
#[into_params(parameter_in = Query)]
pub(super) struct VersionQuery {
    filename: Option<String>,
    content_type: Option<String>,
    note: Option<String>,
}

/// `POST /api/v1/media/{id}/versions`: new version (append-only), becomes
/// the current one.
#[utoipa::path(
    post,
    path = "/api/v1/media/{id}/versions",
    tag = "media",
    operation_id = "media_versions_create",
    summary = "Upload a new version of a media asset",
    description = "Appends version n+1 from the raw request body (`application/octet-stream`) and makes it current; older versions are kept. Owner/admin only.",
    params(("id" = String, Path, format = "uuid", description = "Media asset UUID"), VersionQuery),
    request_body(content = Vec<u8>, content_type = "application/octet-stream", description = "File bytes"),
    responses(
        (status = 201, body = MediaAssetDto),
        (status = 400, description = "Format not allowed (`media-format-unsupported`) or empty body (`field-invalid`, `filename`: `required`)", body = crate::openapi::ErrorDetail),
        (status = 403, description = "Viewer role (`media-write-forbidden`)", body = crate::openapi::ErrorDetail),
        (status = 404, description = "Unknown asset or asset of another organisation", body = crate::openapi::ErrorDetail),
        (status = 413, description = "Body over the upload limit (`media-file-too-large`)", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
pub(super) async fn add_version(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(id): Path<Uuid>,
    Query(q): Query<VersionQuery>,
    body: Bytes,
) -> Result<Response> {
    if !org.can_write() {
        return Err(forbidden(
            "media-write-forbidden",
            "Media writes are restricted to owner and admin roles",
        ));
    }
    let Some(asset) = find_asset(&ctx.db, &org, id).await? else {
        return Err(Error::NotFound);
    };
    let settings = MediaSettings::from_config(&ctx.config);
    if body.is_empty() {
        return Ok(field_status(
            "filename",
            pnex_core::err_codes::FIELD_REQUIRED,
        ));
    }
    if body.len() > settings.max_bytes {
        return Ok(too_large(&settings));
    }
    let filename = sanitize_segment(
        q.filename
            .as_deref()
            .map(str::trim)
            .filter(|f| !f.is_empty())
            .unwrap_or("media.bin"),
    );
    let content_type = q
        .content_type
        .as_deref()
        .map(str::trim)
        .filter(|c| !c.is_empty())
        .unwrap_or("application/octet-stream")
        .to_string();
    write_version(
        &ctx,
        org.org.id,
        &asset,
        IncomingVersion {
            filename,
            content_type,
            note: q.note.filter(|n| !n.trim().is_empty()),
            bytes: body,
        },
        &settings,
    )
    .await?;
    detail_response(&ctx, &org, asset.id, StatusCode::CREATED).await
}

// ─────────────────── GET/DELETE /media/{id}/versions/{n} ───────────────────

#[derive(Deserialize)]
pub(super) struct VersionPath {
    pub(super) id: Uuid,
    pub(super) n: i64,
}

/// Page parameters of the versions list.
#[derive(Deserialize, utoipa::IntoParams)]
#[into_params(parameter_in = Query)]
pub(super) struct VersionsPage {
    #[param(value_type = Option<i64>)]
    pub(super) limit: Option<String>,
    #[param(value_type = Option<i64>)]
    pub(super) offset: Option<String>,
}

/// `GET /api/v1/media/{id}/versions`: paginated versions, oldest first.
#[utoipa::path(
    get,
    path = "/api/v1/media/{id}/versions",
    tag = "media",
    operation_id = "media_versions_list",
    summary = "List the versions of a media asset",
    description = "Paginated versions (`limit` default 10, max 100; `offset`), oldest first; `current` marks the current one.",
    params(("id" = String, Path, format = "uuid", description = "Media asset UUID"), VersionsPage),
    responses(
        (status = 200, body = pnex_core::Paginated<MediaVersionDto>),
        (status = 404, description = "Unknown asset or asset of another organisation", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
pub(super) async fn versions(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(id): Path<Uuid>,
    Query(q): Query<VersionsPage>,
) -> Result<Response> {
    let Some(asset) = find_asset(&ctx.db, &org, id).await? else {
        return Err(Error::NotFound);
    };
    let page = pagination::PageParams::from(q.limit.as_deref(), q.offset.as_deref());
    let select = media_versions::Entity::find()
        .filter(media_versions::Column::AssetId.eq(asset.id))
        .order_by_asc(media_versions::Column::VersionNumber);
    let (count, rows) = pagination::sql_page(&ctx.db, select, page)
        .await
        .map_err(|_| Error::InternalServerError)?;
    let results: Vec<MediaVersionDto> = rows
        .iter()
        .map(|v| version_dto(v, asset.current_version_id))
        .collect();
    Ok(format::json(pagination::envelope(
        &format!("/api/v1/media/{id}/versions"),
        &[],
        page,
        count,
        results,
    ))
    .into_response())
}

/// Version `n` of the asset, else None.
pub(super) async fn find_version(
    db: &DatabaseConnection,
    asset_id: Uuid,
    n: i64,
) -> Result<Option<media_versions::Model>> {
    media_versions::Entity::find()
        .filter(media_versions::Column::AssetId.eq(asset_id))
        .filter(media_versions::Column::VersionNumber.eq(n))
        .one(db)
        .await
        .map_err(|_| Error::InternalServerError)
}

/// `DELETE /api/v1/media/{id}/versions/{n}`: purges the version and its
/// blob; 409 `media-last-version` when it is the only one left (an asset
/// always keeps at least one version).
#[utoipa::path(
    delete,
    path = "/api/v1/media/{id}/versions/{n}",
    tag = "media",
    operation_id = "media_versions_delete",
    summary = "Delete one version of a media asset",
    description = "Deletes the version and its stored file. If it was current, the highest remaining version becomes current. The last remaining version cannot be deleted (delete the asset instead). Owner/admin only.",
    params(("id" = String, Path, format = "uuid", description = "Media asset UUID"), ("n" = i64, Path, description = "Version number (1-based)")),
    responses(
        (status = 204, description = "Deleted"),
        (status = 403, description = "Viewer role (`media-write-forbidden`)", body = crate::openapi::ErrorDetail),
        (status = 404, description = "Unknown asset or version", body = crate::openapi::ErrorDetail),
        (status = 409, description = "Last remaining version (`media-last-version`)", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
pub(super) async fn delete_version(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(path): Path<VersionPath>,
) -> Result<Response> {
    if !org.can_write() {
        return Err(forbidden(
            "media-write-forbidden",
            "Media writes are restricted to owner and admin roles",
        ));
    }
    let Some(asset) = find_asset(&ctx.db, &org, path.id).await? else {
        return Err(Error::NotFound);
    };
    let all = versions_of(&ctx.db, asset.id).await?;
    if all.len() <= 1 {
        return Err(conflict(
            "media-last-version",
            "The last remaining version cannot be deleted — delete the whole asset instead (DELETE /media/{id})",
        ));
    }
    let Some(version) = all.iter().find(|v| v.version_number == path.n) else {
        return Err(Error::NotFound);
    };
    let was_current = asset.current_version_id == Some(version.id);
    version
        .clone()
        .delete(&ctx.db)
        .await
        .map_err(|_| Error::InternalServerError)?;
    // Si c'était la version courante : la plus haute restante devient courante.
    if was_current {
        let next_current = all
            .iter()
            .filter(|v| v.version_number != path.n)
            .map(|v| v.version_number)
            .max()
            .and_then(|max| all.iter().find(|v| v.version_number == max));
        if let Some(next) = next_current {
            let mut a: media_assets::ActiveModel = asset.into();
            a.current_version_id = Set(Some(next.id));
            a.updated_at = Set(chrono::Utc::now().into());
            a.update(&ctx.db)
                .await
                .map_err(|_| Error::InternalServerError)?;
        }
    }
    purge_blobs(&ctx, vec![version.storage_key.clone()]).await;
    Ok(StatusCode::NO_CONTENT.into_response())
}

/// `POST /api/v1/media/{id}/versions/{n}/restore`: moves the current
/// version (flows /rollback school).
#[utoipa::path(
    post,
    path = "/api/v1/media/{id}/versions/{n}/restore",
    tag = "media",
    operation_id = "media_versions_restore",
    summary = "Make an older version current",
    description = "Points the asset's current version at version n; no version is created or deleted. Owner/admin only.",
    params(("id" = String, Path, format = "uuid", description = "Media asset UUID"), ("n" = i64, Path, description = "Version number (1-based)")),
    responses(
        (status = 200, body = MediaAssetDto),
        (status = 403, description = "Viewer role (`media-write-forbidden`)", body = crate::openapi::ErrorDetail),
        (status = 404, description = "Unknown asset or version", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
pub(super) async fn restore(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(path): Path<VersionPath>,
) -> Result<Response> {
    if !org.can_write() {
        return Err(forbidden(
            "media-write-forbidden",
            "Media writes are restricted to owner and admin roles",
        ));
    }
    let Some(asset) = find_asset(&ctx.db, &org, path.id).await? else {
        return Err(Error::NotFound);
    };
    let Some(version) = find_version(&ctx.db, asset.id, path.n).await? else {
        return Err(Error::NotFound);
    };
    let mut a: media_assets::ActiveModel = asset.into();
    a.current_version_id = Set(Some(version.id));
    a.updated_at = Set(chrono::Utc::now().into());
    let asset = a
        .update(&ctx.db)
        .await
        .map_err(|_| Error::InternalServerError)?;
    let versions = versions_of(&ctx.db, asset.id).await?;
    Ok(format::json(asset_dto(&asset, &versions)).into_response())
}

// ─────────────────── GET /media/{id}/versions/diff (D311) ───────────────────

#[derive(Deserialize, utoipa::IntoParams)]
#[into_params(parameter_in = Query)]
pub(super) struct DiffQuery {
    /// Older version number.
    from: i64,
    /// Newer version number.
    to: i64,
}

/// Readable text of a stored version (documents only).
async fn version_text(ctx: &AppContext, v: &media_versions::Model) -> Result<Option<String>> {
    use pnex_core::doc_extract;
    let settings = MediaSettings::from_config(&ctx.config);
    let store = settings.store().map_err(|_| Error::InternalServerError)?;
    let bytes = store
        .get(&v.storage_key)
        .await
        .map_err(|_| Error::NotFound)?;
    let filename = v.filename.clone();
    let job = tokio::task::spawn_blocking(move || {
        let format = doc_extract::format_of(&filename, &bytes)?;
        doc_extract::extract(format, &bytes, &doc_extract::Limits::default()).ok()
    });
    let doc = tokio::time::timeout(std::time::Duration::from_secs(30), job)
        .await
        .ok()
        .and_then(Result::ok)
        .flatten();
    Ok(doc.map(|d| {
        d.sections
            .into_iter()
            .map(|s| s.text)
            .collect::<Vec<_>>()
            .join("\n\n")
    }))
}

/// `GET /api/v1/media/{id}/versions/diff?from=&to=`: paragraph diff of two
/// versions of a document, computed on demand (1 MB of text per side).
#[utoipa::path(
    get,
    path = "/api/v1/media/{id}/versions/diff",
    tag = "media",
    operation_id = "media_versions_diff",
    summary = "Compare two versions of a document",
    description = "Paragraph diff of the readable text of versions `from` and `to` of a document (text, Markdown, CSV, Word, PDF, sheets): paragraphs added, removed and changed with their positions. Computed on demand, 1 MB of text per side at most (`truncated` beyond). Any org member.",
    params(("id" = String, Path, format = "uuid", description = "Media asset UUID"), DiffQuery),
    responses(
        (status = 200, body = pnex_core::text_diff::TextDiff),
        (status = 404, description = "Unknown asset or version", body = crate::openapi::ErrorDetail),
        (status = 422, description = "Not a readable document (`media-diff-unsupported`)", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
pub(super) async fn versions_diff(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(id): Path<Uuid>,
    Query(q): Query<DiffQuery>,
) -> Result<Response> {
    let Some(asset) = find_asset(&ctx.db, &org, id).await? else {
        return Err(Error::NotFound);
    };
    let (Some(a), Some(b)) = (
        find_version(&ctx.db, asset.id, q.from).await?,
        find_version(&ctx.db, asset.id, q.to).await?,
    ) else {
        return Err(Error::NotFound);
    };
    let (Some(old), Some(new)) = (version_text(&ctx, &a).await?, version_text(&ctx, &b).await?)
    else {
        return Err(crate::controllers::coded_error(
            StatusCode::UNPROCESSABLE_ENTITY,
            pnex_core::err_codes::MEDIA_DIFF_UNSUPPORTED,
            "Only documents whose text can be read are compared.",
            None,
        ));
    };
    let mut diff = pnex_core::text_diff::diff(&old, &new);
    diff.from_version = q.from;
    diff.to_version = q.to;
    Ok(format::json(diff).into_response())
}
