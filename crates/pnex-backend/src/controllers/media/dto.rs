use super::*;

// ─────────────────────────── DTOs ───────────────────────────

/// One version of a media asset.
#[derive(serde::Serialize, utoipa::ToSchema)]
pub(super) struct MediaVersionDto {
    #[schema(value_type = String, format = "uuid")]
    id: Uuid,
    version_number: i64,
    filename: String,
    content_type: String,
    size_bytes: i64,
    sha256: Option<String>,
    metadata: Option<serde_json::Value>,
    note: Option<String>,
    current: bool,
    #[schema(format = DateTime)]
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

/// Kind of a media asset (closed set, stored as its wire string).
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize, utoipa::ToSchema,
)]
#[serde(rename_all = "snake_case")]
pub(super) enum MediaKind {
    Photo,
    /// Equirectangular 360 photo.
    Panorama,
    /// Gaussian splat scene.
    Splat,
    /// Imported floor plan (referenced by `tour_versions.doc`).
    Floorplan,
    /// ONNX model of the vision registry (camera-video.md D81).
    Model,
    /// Indexed for search (doc-search.md P1).
    Document,
    /// Indexed for search (doc-search.md P1).
    Table,
    /// Voice clips (voice-id.md: enrolment of a voice profile).
    Audio,
    /// MP4 / MOV clips (Vision Lab frames, media-vision-studio.md).
    Video,
    /// GLB 3D model of an object's gallery (assisted-creation.md D231).
    Mesh,
}

impl MediaKind {
    /// Kind of its wire string (`photo`, `panorama`…), `None` when unknown.
    pub(super) fn parse(raw: &str) -> Option<Self> {
        serde_json::from_value(serde_json::Value::String(raw.to_string())).ok()
    }
}

/// A media asset (versions summarised: count, total size, current one).
#[derive(serde::Serialize, utoipa::ToSchema)]
pub(super) struct MediaAssetDto {
    #[schema(value_type = String, format = "uuid")]
    id: Uuid,
    kind: MediaKind,
    name: String,
    description: Option<String>,
    metadata: Option<serde_json::Value>,
    /// Own labels (D42); effective ones via `/resources/{kind}/{id}/labels/effective`.
    pub(super) labels: pnex_core::resources::LabelSet,
    latest_version_number: i64,
    size_bytes: i64,
    current_version_number: Option<i64>,
    /// Content type of the current version (data URI on the native frontend).
    content_type: Option<String>,
    versions_count: i64,
    #[schema(format = DateTime)]
    created_at: String,
    #[schema(format = DateTime)]
    updated_at: String,
}

/// Summary DTO of an asset from its versions (count, total size, current).
pub(super) fn asset_dto(
    a: &media_assets::Model,
    versions: &[media_versions::Model],
) -> MediaAssetDto {
    let latest = versions.iter().map(|v| v.version_number).max();
    let total: i64 = versions.iter().map(|v| v.size_bytes).sum();
    MediaAssetDto {
        id: a.id,
        // Only validated kinds are ever written (upload, sniff, internal
        // producers); an unknown row would be a bug, shown as a photo.
        kind: MediaKind::parse(&a.kind).unwrap_or(MediaKind::Photo),
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
