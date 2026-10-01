use super::*;

// ─────────────────────────── POST /media ───────────────────────────

#[derive(Deserialize)]
pub(super) struct UploadQuery {
    name: Option<String>,
    filename: Option<String>,
    kind: Option<String>,
    content_type: Option<String>,
}

/// Kinds applicatifs — extension sans migration (kind = string). Studio :
/// `floorplan` = plan d'étage importé (référencé par `tour_versions.doc`).
/// `model` = ONNX model of the vision registry (camera-video.md D81).
const KINDS: [&str; 5] = ["photo", "panorama", "splat", "floorplan", "model"];

/// `POST /api/v1/media` — upload octet-stream, crée asset + version 1.
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
        return Ok(field_status("filename", "corps de la requête vide"));
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
    // Kind : client > sniff > photo — validation stricte.
    let sniff = media_sniff::detect(&content_type_of(&q), &filename, &body);
    let kind = q.kind.as_deref().map(str::trim).filter(|k| !k.is_empty());
    if let Some(k) = kind {
        if !KINDS.contains(&k) {
            return Ok(field_status(
                "kind",
                "kind inconnu (photo | panorama | splat | floorplan | model)",
            ));
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
    // Retourne le détail complet (asset + versions).
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

/// 413 JSON propre (la limite axum, posée à max+1 Mo, n'a pas dû bouger).
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

#[derive(Deserialize)]
pub(super) struct ListQuery {
    kind: Option<String>,
    search: Option<String>,
    /// Filtre label **effectif** `name` ou `name:valeur` (D42 — propres ou
    /// hérités d'un ancêtre du containment).
    label: Option<String>,
    limit: Option<String>,
    offset: Option<String>,
}

/// `GET /api/v1/media?label=site:serre` — liste paginée (D14), filtres kind
/// + search (nom) + label effectif (D42).
pub(super) async fn list(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Query(q): Query<ListQuery>,
) -> Result<Response> {
    let page = pagination::PageParams::from(q.limit.as_deref(), q.offset.as_deref());
    let mut query = media_assets::Entity::find()
        .filter(media_assets::Column::OrgId.eq(org.org.id))
        .order_by_desc(media_assets::Column::UpdatedAt);
    // kind accepte une liste séparée par virgules (picker plan du studio :
    // floorplan + photo — n'importe quelle image convient comme plan).
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
    // D42 : filtre par labels EFFECTIFS (propres ou hérités d'un ancêtre) —
    // le média dans le folder « Serre » taggé site:serre sort du filtre.
    let label_ids: Option<std::collections::HashSet<String>> = match q.label.as_deref() {
        None => None,
        Some(raw) => {
            let (filter, err) = pnex_core::resources::parse_label_filter(raw);
            let Some(filter) = filter else {
                return Ok(field_status(
                    "label",
                    err.as_deref().unwrap_or("label invalide"),
                ));
            };
            let ids = crate::services::resources::labels::ids_with_effective_label(
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

/// Réponse détail complète (asset + versions), réutilisée par upload/add.
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

/// `GET /api/v1/media/{id}` — détail + versions.
pub(super) async fn detail(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(id): Path<Uuid>,
) -> Result<Response> {
    detail_response(&ctx, &org, id, StatusCode::OK).await
}

// ─────────────────────── PATCH /media/{id} ───────────────────────

#[derive(Deserialize)]
pub(super) struct PatchAsset {
    name: Option<String>,
    description: Option<String>,
}

/// `PATCH /api/v1/media/{id}` — rename/description.
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
            return Ok(field_status("name", "le nom ne peut pas être vide"));
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

/// `DELETE /api/v1/media/{id}` — purge l'asset, ses versions et tous les
/// blobs (storage best-effort : la purge DB est la source de vérité).
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

/// Purge best-effort des blobs (delete idempotent) — hors chemin de réponse.
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
