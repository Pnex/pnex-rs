use super::*;

// ─────────────────────────── POST /media ───────────────────────────

#[derive(Deserialize, utoipa::IntoParams)]
#[into_params(parameter_in = Query)]
pub(super) struct UploadQuery {
    name: Option<String>,
    filename: Option<String>,
    kind: Option<String>,
    content_type: Option<String>,
}

/// `POST /api/v1/media`: octet-stream upload, creates the asset + version 1.
#[utoipa::path(
    post,
    path = "/api/v1/media",
    tag = "media",
    operation_id = "media_upload",
    summary = "Upload a new media asset",
    description = "Creates an asset and its version 1 from the raw request body (`application/octet-stream`, no multipart); metadata travel in the query. The kind is taken from `kind` when given, else sniffed from content and extension; the content must be an allowed format whose extension matches it. Stored in object storage; owner/admin only. Concurrent large uploads are queued per server.",
    params(UploadQuery),
    request_body(content = Vec<u8>, content_type = "application/octet-stream", description = "File bytes"),
    responses(
        (status = 201, body = MediaAssetDto),
        (status = 400, description = "Format not allowed or mismatching extension (`media-format-unsupported`), or `field-invalid` (`filename`: `required` for an empty body, `kind`: `invalid`)", body = crate::openapi::ErrorDetail),
        (status = 403, description = "Viewer role (`media-write-forbidden`)", body = crate::openapi::ErrorDetail),
        (status = 413, description = "Body over the upload limit (`media-file-too-large`, `errors.args.max_bytes`)", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
pub(super) async fn upload(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Query(q): Query<UploadQuery>,
    body: Bytes,
) -> Result<Response> {
    if !org.can_write() {
        return Err(forbidden(
            "media-write-forbidden",
            "Media writes are restricted to owner and admin roles",
        ));
    }
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
    let name = q
        .name
        .as_deref()
        .map(str::trim)
        .filter(|n| !n.is_empty())
        .map_or_else(|| filename.clone(), str::to_string);
    // Kind: client > sniff; the content must be an allowed format whose
    // extension agrees, and fit the forced kind (media_sniff).
    let Some(sniff) = media_sniff::detect(&filename, &body) else {
        return Err(format_unsupported());
    };
    let kind = q.kind.as_deref().map(str::trim).filter(|k| !k.is_empty());
    if let Some(k) = kind {
        if MediaKind::parse(k).is_none() {
            return Ok(field_status("kind", pnex_core::err_codes::FIELD_INVALID));
        }
        if !media_sniff::accepts(k, &filename, &body) {
            return Err(format_unsupported());
        }
    }
    let kind = kind.map_or_else(|| sniff.kind.to_string(), str::to_string);
    let asset = media_assets::ActiveModel {
        id: Set(Uuid::new_v4()),
        org_id: Set(org.org.id),
        kind: Set(kind),
        name: Set(name),
        description: Set(None),
        metadata: Set(None),
        ..Default::default()
    }
    .insert(&ctx.db)
    .await
    .map_err(|_| Error::InternalServerError)?;
    let _version = write_version(
        &ctx,
        org.org.id,
        &asset,
        IncomingVersion {
            filename,
            content_type: content_type_of(&q),
            note: None,
            bytes: body,
        },
        &settings,
    )
    .await?;
    detail_response(&ctx, &org, asset.id, StatusCode::CREATED).await
}

fn content_type_of(q: &UploadQuery) -> String {
    q.content_type
        .as_deref()
        .map(str::trim)
        .filter(|c| !c.is_empty())
        .unwrap_or("application/octet-stream")
        .to_string()
}

/// Coded 413 (the axum limit, set at max + 1 MB, is only the net).
/// Machine-coded: the byte ceiling travels in `errors.args` (never
/// pre-rendered into the description).
pub(super) fn too_large(settings: &MediaSettings) -> Response {
    Error::CustomError(
        StatusCode::PAYLOAD_TOO_LARGE,
        loco_rs::controller::ErrorDetail {
            error: Some("media-file-too-large".to_string()),
            description: Some(
                "File exceeds the upload size limit (PNEX_MEDIA_MAX_BYTES)".to_string(),
            ),
            errors: Some(serde_json::json!({
                "args": { "max_bytes": settings.max_bytes.to_string() }
            })),
        },
    )
    .into_response()
}

// ─────────────────────────── GET /media ───────────────────────────

#[derive(Deserialize, utoipa::IntoParams)]
#[into_params(parameter_in = Query)]
pub(super) struct ListQuery {
    kind: Option<String>,
    search: Option<String>,
    /// **Effective** label filter `name` or `name:value` (D42: own labels or
    /// inherited from a containment ancestor).
    label: Option<String>,
    #[param(value_type = Option<i64>)]
    limit: Option<String>,
    #[param(value_type = Option<i64>)]
    offset: Option<String>,
}

/// `GET /api/v1/media?label=site:greenhouse`: paginated list (D14), filters
/// kind + search (name) + effective label (D42).
#[utoipa::path(
    get,
    path = "/api/v1/media",
    tag = "media",
    operation_id = "media_list",
    summary = "List the media assets of the organisation",
    description = "Paginated (`limit` default 10, max 100; `offset`), most recently updated first. Filters: `kind` (comma-separated list), `search` (name contains), `label` (effective label `name` or `name:value`, inherited ones included).",
    params(ListQuery),
    responses(
        (status = 200, body = pnex_core::Paginated<MediaAssetDto>),
        (status = 400, description = "Invalid `label` filter (`field-invalid`, `label`: `invalid`)", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
pub(super) async fn list(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Query(q): Query<ListQuery>,
) -> Result<Response> {
    let page = pagination::PageParams::from(q.limit.as_deref(), q.offset.as_deref());
    let mut query = media_assets::Entity::find()
        .filter(media_assets::Column::OrgId.eq(org.org.id))
        .order_by_desc(media_assets::Column::UpdatedAt);
    // `kind` takes a comma-separated list (studio plan picker: floorplan +
    // photo, any image works as a plan).
    let kinds: Vec<&str> = q
        .kind
        .as_deref()
        .unwrap_or("")
        .split(',')
        .map(str::trim)
        .filter(|k| !k.is_empty())
        .collect();
    if !kinds.is_empty() {
        query = query.filter(media_assets::Column::Kind.is_in(kinds));
    }
    // D42: filter on EFFECTIVE labels (own or inherited from an ancestor).
    let label_ids: Option<std::collections::HashSet<String>> = match q.label.as_deref() {
        None => None,
        Some(raw) => {
            let (filter, _) = pnex_core::resources::parse_label_filter(raw);
            let Some(filter) = filter else {
                return Ok(field_status("label", pnex_core::err_codes::FIELD_INVALID));
            };
            let ids = crate::services::resources::labels::ids_with_label(
                &ctx.db,
                org.org.id,
                &filter,
                Some(pnex_core::resources::KIND_MEDIA_ASSET),
            )
            .await
            .map_err(|_| Error::InternalServerError)?;
            Some(ids.into_iter().map(|(_, id)| id).collect())
        }
    };

    // Search, label set, COUNT and LIMIT/OFFSET run in SQL (org-scoped).
    if let Some(pat) = pagination::sql_search_pattern(q.search.as_deref()) {
        query = query.filter(pagination::sql_contains(
            (media_assets::Entity, media_assets::Column::Name),
            &pat,
        ));
    }
    if let Some(ids) = &label_ids {
        let ids: Vec<Uuid> = ids.iter().filter_map(|id| id.parse().ok()).collect();
        query = query.filter(media_assets::Column::Id.is_in(ids));
    }
    let (count, page_rows) = pagination::sql_page(&ctx.db, query, page)
        .await
        .map_err(|_| Error::InternalServerError)?;

    // Hydratation en vrac (pas de N+1) : toutes les versions de la page.
    let ids: Vec<Uuid> = page_rows.iter().map(|a| a.id).collect();
    let mut by_asset: std::collections::HashMap<Uuid, Vec<media_versions::Model>> =
        std::collections::HashMap::new();
    if !ids.is_empty() {
        let versions = media_versions::Entity::find()
            .filter(media_versions::Column::AssetId.is_in(ids.clone()))
            .all(&ctx.db)
            .await
            .map_err(|_| Error::InternalServerError)?;
        for v in versions {
            by_asset.entry(v.asset_id).or_default().push(v);
        }
    }
    // D42 : labels de la page en 1 requête (pas de N+1).
    let label_map = crate::services::resources::labels::labels_for_many(
        &ctx.db,
        org.org.id,
        pnex_core::resources::KIND_MEDIA_ASSET,
        &ids.iter().map(|id| id.to_string()).collect::<Vec<_>>(),
    )
    .await
    .unwrap_or_default();
    let results: Vec<MediaAssetDto> = page_rows
        .iter()
        .map(|a| {
            let empty: Vec<media_versions::Model> = Vec::new();
            let mut dto = asset_dto(a, by_asset.get(&a.id).unwrap_or(&empty));
            dto.labels = label_map
                .get(&a.id.to_string())
                .cloned()
                .unwrap_or_default();
            dto
        })
        .collect();
    let filters: Vec<(String, String)> = [
        ("kind", q.kind.clone()),
        ("search", q.search.clone()),
        ("label", q.label.clone()),
    ]
    .into_iter()
    .filter_map(|(k, v)| v.map(|v| (k.to_string(), v)))
    .collect();
    Ok(format::json(pagination::envelope(
        "/api/v1/media",
        &filters,
        page,
        count,
        results,
    ))
    .into_response())
}

// ─────────────────────── GET /media/{id} ───────────────────────

/// Asset summary response (`MediaAssetDto`), shared by detail, upload and
/// add_version.
pub(super) async fn detail_response(
    ctx: &AppContext,
    org: &OrgContext,
    id: Uuid,
    status: StatusCode,
) -> Result<Response> {
    let Some(asset) = find_asset(&ctx.db, org, id).await? else {
        return Err(Error::NotFound);
    };
    let versions = versions_of(&ctx.db, asset.id).await?;
    Ok((status, format::json(asset_dto(&asset, &versions))).into_response())
}

/// `GET /api/v1/media/{id}`: asset summary.
#[utoipa::path(
    get,
    path = "/api/v1/media/{id}",
    tag = "media",
    operation_id = "media_get",
    summary = "Get a media asset",
    description = "Asset summary: kind, name, versions count, total size, current version number and content type.",
    params(("id" = String, Path, format = "uuid", description = "Media asset UUID")),
    responses(
        (status = 200, body = MediaAssetDto),
        (status = 404, description = "Unknown asset or asset of another organisation", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
pub(super) async fn detail(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(id): Path<Uuid>,
) -> Result<Response> {
    detail_response(&ctx, &org, id, StatusCode::OK).await
}

// ─────────────────────── PATCH /media/{id} ───────────────────────

#[derive(Deserialize, utoipa::ToSchema)]
pub(super) struct PatchAsset {
    name: Option<String>,
    description: Option<String>,
}

/// `PATCH /api/v1/media/{id}`: rename / description.
#[utoipa::path(
    patch,
    path = "/api/v1/media/{id}",
    tag = "media",
    operation_id = "media_update",
    summary = "Rename a media asset or change its description",
    description = "Absent fields are left unchanged. Owner/admin only.",
    params(("id" = String, Path, format = "uuid", description = "Media asset UUID")),
    request_body = PatchAsset,
    responses(
        (status = 200, body = MediaAssetDto),
        (status = 400, description = "Empty name (`field-invalid`, `name`: `required`)", body = crate::openapi::ErrorDetail),
        (status = 403, description = "Viewer role (`media-write-forbidden`)", body = crate::openapi::ErrorDetail),
        (status = 404, description = "Unknown asset or asset of another organisation", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
pub(super) async fn update(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(id): Path<Uuid>,
    Json(patch): Json<PatchAsset>,
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
    let mut asset: media_assets::ActiveModel = asset.into();
    if let Some(name) = patch.name.map(|n| n.trim().to_string()) {
        if name.is_empty() {
            return Ok(field_status("name", pnex_core::err_codes::FIELD_REQUIRED));
        }
        asset.name = Set(name);
    }
    if let Some(description) = patch.description {
        asset.description = Set(Some(description));
    }
    asset.updated_at = Set(chrono::Utc::now().into());
    let asset = asset
        .update(&ctx.db)
        .await
        .map_err(|_| Error::InternalServerError)?;
    let versions = versions_of(&ctx.db, asset.id).await?;
    Ok(format::json(asset_dto(&asset, &versions)).into_response())
}

// ─────────────────────── DELETE /media/{id} ───────────────────────

/// `DELETE /api/v1/media/{id}`: purges the asset, its versions and every
/// blob (storage best effort: the DB purge is the source of truth).
#[utoipa::path(
    delete,
    path = "/api/v1/media/{id}",
    tag = "media",
    operation_id = "media_delete",
    summary = "Delete a media asset",
    description = "Permanently deletes the asset, all its versions and their stored files, and its labels, containment and links. Owner/admin only.",
    params(("id" = String, Path, format = "uuid", description = "Media asset UUID")),
    responses(
        (status = 204, description = "Deleted"),
        (status = 403, description = "Viewer role (`media-write-forbidden`)", body = crate::openapi::ErrorDetail),
        (status = 404, description = "Unknown asset or asset of another organisation", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
pub(super) async fn delete(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(id): Path<Uuid>,
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
    let versions = versions_of(&ctx.db, asset.id).await?;
    let keys: Vec<String> = versions.iter().map(|v| v.storage_key.clone()).collect();
    // D42 : purge **symétrique** de la couche d'organisation (labels,
    // containment, arêtes des deux bouts) avant le delete.
    crate::services::resources::purge_for(
        &ctx.db,
        org.org.id,
        pnex_core::resources::KIND_MEDIA_ASSET,
        &asset.id.to_string(),
    )
    .await
    .map_err(|_| Error::InternalServerError)?;
    asset
        .delete(&ctx.db)
        .await
        .map_err(|_| Error::InternalServerError)?;
    purge_blobs(&ctx, keys).await;
    Ok(StatusCode::NO_CONTENT.into_response())
}

/// Best-effort blob purge (idempotent deletes).
pub(super) async fn purge_blobs(ctx: &AppContext, keys: Vec<String>) {
    if keys.is_empty() {
        return;
    }
    let settings = MediaSettings::from_config(&ctx.config);
    if let Ok(store) = settings.store() {
        for key in keys {
            let _ = store.delete(&key).await;
        }
    }
}
