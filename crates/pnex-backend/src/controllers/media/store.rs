use super::*;

// ─────────────────────── Écriture versionnée ───────────────────────

/// Contenu d'une nouvelle version (regroupé pour l'arity de write_version).
/// `pub(crate)` : réutilisé par le worker de stitch serveur (Take 360 V2),
/// qui publie son panorama HD comme version n+1 de l'asset.
pub(crate) struct IncomingVersion {
    pub filename: String,
    pub content_type: String,
    pub note: Option<String>,
    /// Ref-counted body: moved to the store without copying.
    pub bytes: axum::body::Bytes,
}

/// Écrit une nouvelle version : **storage d'abord, DB ensuite** (un blob
/// orphelin est acceptable/purgeable, une ligne sans blob ne l'est pas) ;
/// en cas d'échec DB, le blob fraîchement écrit est supprimé (best effort).
/// La nouvelle version devient la version courante de l'asset.
///
/// `pub(crate)` + `org_id` en paramètre (et non `&OrgContext`) : le worker
/// de stitch tourne hors requête HTTP — il ne porte qu'un `org_id` relu du
/// job. Écart assumé à la forme d'origine pour éviter la duplication.
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
    // Version n+1 (école flow_versions : incrémental par asset).
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
        // Allowlisted at write time (SEC-5): the stored type is what is served.
        content_type: Set(crate::services::media::safe_content_type(&content_type).to_string()),
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
    // La nouvelle version devient courante.
    let mut a: media_assets::ActiveModel = asset.clone().into();
    a.current_version_id = Set(Some(version.id));
    a.updated_at = Set(chrono::Utc::now().into());
    a.update(&txn)
        .await
        .map_err(|_| Error::InternalServerError)?;
    txn.commit().await.map_err(|e| {
        // DB en échec après put : purge du blob fraîchement écrit (best
        // effort) — pas de ligne sans blob.
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
