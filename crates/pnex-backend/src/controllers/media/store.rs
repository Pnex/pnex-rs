use super::*;

// ─────────────────────── Versioned write ───────────────────────

/// Content of a new version (grouped for write_version's arity).
/// `pub(crate)`: reused by the server stitch worker (Take 360 V2),
/// which publishes its HD panorama as version n+1 of the asset.
pub(crate) struct IncomingVersion {
    pub filename: String,
    pub content_type: String,
    pub note: Option<String>,
    /// Ref-counted body: moved to the store without copying.
    pub bytes: axum::body::Bytes,
}

/// Writes a new version: **storage first, DB next** (an orphan blob
/// is acceptable/purgeable, a row without a blob is not);
/// on DB failure, the freshly written blob is deleted (best effort).
/// The new version becomes the asset's current version.
///
/// `pub(crate)` + `org_id` as a parameter (and not `&OrgContext`): the stitch
/// worker runs outside an HTTP request — it only carries an `org_id` re-read
/// from the job. Deliberate deviation from the original shape to avoid duplication.
pub(crate) async fn write_version(
    ctx: &AppContext,
    org_id: i64,
    asset: &media_assets::Model,
    incoming: IncomingVersion,
    settings: &MediaSettings,
) -> Result<media_versions::Model> {
    let IncomingVersion {
        filename,
        content_type,
        note,
        bytes,
    } = incoming;
    // Per-version metadata (GPano); the content must fit the asset's kind.
    let sniff = media_sniff::detect(&filename, &bytes)
        .filter(|_| media_sniff::accepts(&asset.kind, &filename, &bytes))
        .ok_or_else(super::helpers::format_unsupported)?;
    let sha = sha256_hex(&bytes);
    // Version n+1 (flow_versions school: incremental per asset).
    let next: i64 = media_versions::Entity::find()
        .filter(media_versions::Column::AssetId.eq(asset.id))
        .select_only()
        .column_as(media_versions::Column::VersionNumber.max(), "maxv")
        .into_tuple::<Option<i64>>()
        .one(&ctx.db)
        .await
        .map_err(|_| Error::InternalServerError)?
        .flatten()
        .unwrap_or(0)
        + 1;
    let key = MediaSettings::storage_key(org_id, asset.id, next, &filename);
    let store = settings.store().map_err(|_| Error::InternalServerError)?;
    store
        .put(&key, bytes.clone())
        .await
        .map_err(|_| Error::InternalServerError)?;
    let version = media_versions::ActiveModel {
        id: Set(Uuid::new_v4()),
        asset_id: Set(asset.id),
        org_id: Set(org_id),
        version_number: Set(next),
        filename: Set(filename),
        // Allowlisted at write time (SEC-5): the stored type is what is
        // served. No declared type: the one of the identified content.
        content_type: Set(
            crate::services::media::safe_content_type(match content_type.trim() {
                "" | "application/octet-stream" => sniff.mime.unwrap_or(""),
                declared => declared,
            })
            .to_string(),
        ),
        size_bytes: Set(bytes.len() as i64),
        storage_key: Set(key.clone()),
        sha256: Set(Some(sha)),
        metadata: Set(sniff.metadata),
        note: Set(note),
        ..Default::default()
    };
    let txn = ctx
        .db
        .begin()
        .await
        .map_err(|_| Error::InternalServerError)?;
    let version = version
        .insert(&txn)
        .await
        .map_err(|_| Error::InternalServerError)?;
    // The new version becomes current.
    let mut a: media_assets::ActiveModel = asset.clone().into();
    a.current_version_id = Set(Some(version.id));
    a.updated_at = Set(chrono::Utc::now().into());
    a.update(&txn)
        .await
        .map_err(|_| Error::InternalServerError)?;
    txn.commit().await.map_err(|e| {
        // DB failed after put: purge the freshly written blob (best
        // effort) — no row without a blob.
        let store = store.clone();
        let key = key.clone();
        tokio::spawn(async move {
            let _ = store.delete(&key).await;
        });
        let _ = e;
        Error::InternalServerError
    })?;
    if crate::services::doc_search::is_indexed_kind(&asset.kind) {
        // The upload stands even if queuing fails: reindex recovers it (F4).
        if let Err(e) = crate::services::doc_search::enqueue(ctx, org_id, version.id).await {
            tracing::warn!(version = %version.id, error = %e, "document index not queued");
        }
    }
    Ok(version)
}
