use super::*;

// ─────────────────────────── DTOs ───────────────────────────

#[derive(serde::Serialize)]
pub(super) struct MediaVersionDto {
    id: Uuid,
    version_number: i64,
    filename: String,
    content_type: String,
    size_bytes: i64,
    sha256: Option<String>,
    metadata: Option<serde_json::Value>,
    note: Option<String>,
    current: bool,
    created_at: String,
}

pub(super) fn version_dto(v: &media_versions::Model, current_id: Option<Uuid>) -> MediaVersionDto {
    MediaVersionDto {
        id: v.id,
        version_number: v.version_number,
        filename: v.filename.clone(),
        content_type: v.content_type.clone(),
        size_bytes: v.size_bytes,
        sha256: v.sha256.clone(),
        metadata: v.metadata.clone(),
        note: v.note.clone(),
        current: Some(v.id) == current_id,
        created_at: v.created_at.to_rfc3339(),
    }
}

#[derive(serde::Serialize)]
pub(super) struct MediaAssetDto {
    id: Uuid,
    kind: String,
    name: String,
    description: Option<String>,
    metadata: Option<serde_json::Value>,
    /// Labels propres (D42) — effectifs via `/resources/{kind}/{id}/labels/effective`.
    pub(super) labels: pnex_core::resources::LabelSet,
    latest_version_number: i64,
    size_bytes: i64,
    current_version_number: Option<i64>,
    /// Content-type de la version courante (data URI côté front natif).
    content_type: Option<String>,
    versions_count: i64,
    created_at: String,
    updated_at: String,
}

/// DTO résumé pour la liste (hydratation versions_count/size en vrac).
pub(super) fn asset_dto(
    a: &media_assets::Model,
    versions: &[media_versions::Model],
) -> MediaAssetDto {
    let latest = versions.iter().map(|v| v.version_number).max();
    let total: i64 = versions.iter().map(|v| v.size_bytes).sum();
    MediaAssetDto {
        id: a.id,
        kind: a.kind.clone(),
        name: a.name.clone(),
        description: a.description.clone(),
        metadata: a.metadata.clone(),
        labels: Default::default(),
        latest_version_number: latest.unwrap_or(0),
        size_bytes: total,
        current_version_number: a
            .current_version_id
            .and_then(|id| versions.iter().find(|v| v.id == id))
            .map(|v| v.version_number),
        content_type: a
            .current_version_id
            .and_then(|id| versions.iter().find(|v| v.id == id))
            .map(|v| v.content_type.clone()),
        versions_count: versions.len() as i64,
        created_at: a.created_at.to_rfc3339(),
        updated_at: a.updated_at.to_rfc3339(),
    }
}
