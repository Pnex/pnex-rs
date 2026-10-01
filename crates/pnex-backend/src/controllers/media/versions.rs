use super::*;

// ─────────────────────── POST /media/{id}/versions ───────────────────────

#[derive(Deserialize)]
pub(super) struct VersionQuery {
    filename: Option<String>,
    content_type: Option<String>,
    note: Option<String>,
}

/// `POST /api/v1/media/{id}/versions` — nouvelle version (append-only),
/// devient la version courante.
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

/// `GET /api/v1/media/{id}/versions` — liste des versions.
pub(super) async fn versions(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(id): Path<Uuid>,
) -> Result<Response> {
    let Some(asset) = find_asset(&ctx.db, &org, id).await? else {
        return Err(Error::NotFound);
    };
    let rows = versions_of(&ctx.db, asset.id).await?;
    let results: Vec<MediaVersionDto> = rows
        .iter()
        .map(|v| version_dto(v, asset.current_version_id))
        .collect();
    Ok(
        format::json(serde_json::json!({ "count": results.len(), "results": results }))
            .into_response(),
    )
}

/// Version `n` de l'asset, sinon None.
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

/// `DELETE /api/v1/media/{id}/versions/{n}` — purge la version et son blob ;
/// 409 `last_version` si c'est la seule restante (un asset a toujours au
/// moins une version).
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

/// `POST /api/v1/media/{id}/versions/{n}/restore` — re-positionne la version
/// courante (école flows /rollback).
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
