//! Media API (D21) — client of the `/api/v1/media` controller (contract
//! `docs/contracts/media.http`). Octet-stream upload via
//! `client::request_upload` (no multipart), D14 paginated listing.

use serde::{Deserialize, Serialize};

use crate::api::client;
use crate::api::error::ApiError;

/// Media asset kind — mirror of the controller (`photo | panorama | splat`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MediaKind {
    Photo,
    Panorama,
    Splat,
    /// Floor plan — consumed by the base map (viz) and referenced by
    /// `tour_versions.doc` (S9 studio.md); kind = application string.
    Floorplan,
    /// ONNX model backing a vision registry entry (camera-video.md D81).
    Model,
    /// Text document (txt, md, docx, pdf) indexed for search (doc-search.md).
    Document,
    /// Spreadsheet (csv, xlsx, ods) indexed for search (doc-search.md).
    Table,
    /// Voice clip (voice-id.md: enrolment of a voice profile).
    Audio,
    /// MP4 / MOV clip (Vision Lab: frames of a video).
    Video,
    /// GLB 3D model of an object's gallery (assisted-creation.md D231).
    Mesh,
}

impl MediaKind {
    pub fn as_str(self) -> &'static str {
        match self {
            MediaKind::Photo => "photo",
            MediaKind::Panorama => "panorama",
            MediaKind::Splat => "splat",
            MediaKind::Floorplan => "floorplan",
            MediaKind::Model => "model",
            MediaKind::Document => "document",
            MediaKind::Table => "table",
            MediaKind::Audio => "audio",
            MediaKind::Video => "video",
            MediaKind::Mesh => "mesh",
        }
    }
}

/// Version of a media asset.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct MediaVersion {
    pub id: String,
    pub version_number: i64,
    pub filename: String,
    pub content_type: String,
    pub size_bytes: i64,
    pub sha256: Option<String>,
    pub metadata: Option<serde_json::Value>,
    pub note: Option<String>,
    pub current: bool,
    pub created_at: String,
}

/// Media asset (detail = same shape as the 201 upload response).
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct MediaAsset {
    pub id: String,
    pub kind: String,
    pub name: String,
    pub description: Option<String>,
    pub metadata: Option<serde_json::Value>,
    /// Own labels (D42) — effective ones via the cross-cutting layer.
    #[serde(default)]
    pub labels: std::collections::BTreeMap<String, Option<String>>,
    pub latest_version_number: i64,
    pub size_bytes: i64,
    pub current_version_number: Option<i64>,
    pub content_type: Option<String>,
    pub versions_count: i64,
    pub created_at: String,
    pub updated_at: String,
}

impl MediaAsset {
    /// Typed kind (defaults to photo if the server value is unknown).
    pub fn media_kind(&self) -> MediaKind {
        match self.kind.as_str() {
            "panorama" => MediaKind::Panorama,
            "splat" => MediaKind::Splat,
            "floorplan" => MediaKind::Floorplan,
            "model" => MediaKind::Model,
            "document" => MediaKind::Document,
            "table" => MediaKind::Table,
            "audio" => MediaKind::Audio,
            "video" => MediaKind::Video,
            "mesh" => MediaKind::Mesh,
            _ => MediaKind::Photo,
        }
    }
}

/// Asset summary (list = same shape).
pub type MediaAssetSummary = MediaAsset;

/// D14 paginated envelope (next/previous consumed by the V2 UI — Pager
/// only uses count).
#[derive(Clone, Debug, Deserialize)]
#[allow(dead_code)]
pub struct MediaPage {
    pub count: i64,
    pub next: Option<String>,
    pub previous: Option<String>,
    pub results: Vec<MediaAssetSummary>,
}

/// List filters (D14) — encoded as a home-made query string (as in flows.rs).
#[derive(Debug, Default, Clone)]
pub struct MediaFilters {
    /// Wanted kinds (empty = all) — encoded `kind=a,b` for the server.
    pub kinds: Vec<MediaKind>,
    pub search: Option<String>,
    /// Effective label filter `name` or `name:value` (D42).
    pub label: Option<String>,
    pub limit: Option<i64>,
    pub offset: Option<i64>,
}

impl MediaFilters {
    fn to_query(&self) -> String {
        let mut pairs: Vec<String> = Vec::new();
        if !self.kinds.is_empty() {
            let list: Vec<_> = self.kinds.iter().map(|k| k.as_str()).collect();
            pairs.push(format!("kind={}", list.join(",")));
        }
        if let Some(search) = self.search.as_deref().filter(|s| !s.is_empty()) {
            pairs.push(format!("search={}", urlencode(search)));
        }
        if let Some(label) = self.label.as_deref().filter(|l| !l.is_empty()) {
            pairs.push(format!("label={}", urlencode(label)));
        }
        if let Some(limit) = self.limit {
            pairs.push(format!("limit={limit}"));
        }
        if let Some(offset) = self.offset {
            pairs.push(format!("offset={offset}"));
        }
        if pairs.is_empty() {
            String::new()
        } else {
            format!("?{}", pairs.join("&"))
        }
    }
}

/// Minimal percent-encoding (the front has no url crate — as in flows.rs).
pub(crate) fn urlencode(value: &str) -> String {
    let mut out = String::new();
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(byte as char);
            }
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

/// `GET /api/v1/media` — paginated list.
pub async fn list(filters: &MediaFilters) -> Result<MediaPage, ApiError> {
    client::request(
        reqwest::Method::GET,
        &format!("/api/v1/media{}", filters.to_query()),
        None,
    )
    .await
}

/// `GET /api/v1/media`, every page (pickers and name lookups); the
/// filters' own `limit`/`offset` must be left unset.
pub async fn list_all(filters: &MediaFilters) -> Result<Vec<MediaAssetSummary>, ApiError> {
    client::request_all(&format!("/api/v1/media{}", filters.to_query())).await
}

/// `GET /api/v1/media/{id}` — detail + versions.
pub async fn detail(id: &str) -> Result<MediaAsset, ApiError> {
    client::request(reqwest::Method::GET, &format!("/api/v1/media/{id}"), None).await
}

/// `GET /api/v1/media/{id}/versions`: every version, oldest first (all
/// pages).
pub async fn versions(id: &str) -> Result<Vec<MediaVersion>, ApiError> {
    client::request_all(&format!("/api/v1/media/{id}/versions")).await
}

/// One document search hit (`GET /api/v1/media/search`). The snippet wraps
/// matches in ⟦ and ⟧; it is user data, render it as text only.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct SearchHit {
    pub chunk_id: String,
    pub asset_id: String,
    pub asset_name: String,
    pub kind: String,
    pub filename: String,
    pub page: Option<i64>,
    pub heading: Option<String>,
    pub snippet: String,
    pub score: f64,
}

/// `GET /api/v1/media/search?q=&kind=&k=` — lexical search over the
/// current versions of documents and tables.
pub async fn search(
    q: &str,
    kind: Option<MediaKind>,
    k: Option<u32>,
) -> Result<Vec<SearchHit>, ApiError> {
    #[derive(serde::Deserialize)]
    struct Hits {
        hits: Vec<SearchHit>,
    }

    let mut path = format!("/api/v1/media/search?q={}", urlencode(q));
    if let Some(kind) = kind {
        path.push_str(&format!("&kind={}", kind.as_str()));
    }
    if let Some(k) = k {
        path.push_str(&format!("&k={k}"));
    }
    let hits: Hits = client::request(reqwest::Method::GET, &path, None).await?;
    Ok(hits.hits)
}

/// Index state of an asset's current version (`GET /api/v1/media/{id}/index`).
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct IndexState {
    /// `pending | extracting | indexed | error | needs_ocr`.
    pub status: String,
    pub error_code: Option<String>,
    pub page_count: Option<i64>,
    pub chunk_count: i64,
}

/// Offers, step states and index state of an asset's current version
/// (media-import-pipeline.md D213).
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct AnalysesState {
    pub offers: Vec<pnex_core::media_analysis::StepOffer>,
    pub analyses: Vec<pnex_core::media_analysis::MediaAnalysis>,
    pub index: Option<IndexState>,
}

/// `GET /api/v1/media/{id}/analyses`.
pub async fn analyses(id: &str) -> Result<AnalysesState, ApiError> {
    client::request(
        reqwest::Method::GET,
        &format!("/api/v1/media/{}/analyses", urlencode(id)),
        None,
    )
    .await
}

/// `GET /api/v1/media/{id}/transcript` — runs and the segments of one run
/// (the default one without `run`), media-review.md D276.
pub async fn transcript(
    id: &str,
    run: Option<&str>,
) -> Result<pnex_core::media_review::MediaTranscript, ApiError> {
    let query = run
        .map(|r| format!("?run={}", urlencode(r)))
        .unwrap_or_default();
    client::request(
        reqwest::Method::GET,
        &format!("/api/v1/media/{}/transcript{query}", urlencode(id)),
        None,
    )
    .await
}

/// `GET /api/v1/media/{id}/detections` — boxes of the latest detection
/// (D215), positions on the media clock.
pub async fn detections(id: &str) -> Result<pnex_core::media_review::MediaDetections, ApiError> {
    client::request(
        reqwest::Method::GET,
        &format!("/api/v1/media/{}/detections", urlencode(id)),
        None,
    )
    .await
}

/// `POST /api/v1/media/{id}/attributions` — spans to a person (D273).
pub async fn attribute(
    id: &str,
    input: &pnex_core::media_review::AttributionInput,
) -> Result<serde_json::Value, ApiError> {
    let body = serde_json::to_value(input).map_err(|e| ApiError::new(e.to_string()))?;
    client::request(
        reqwest::Method::POST,
        &format!("/api/v1/media/{}/attributions", urlencode(id)),
        Some(body),
    )
    .await
}

/// `PATCH /api/v1/media/{id}/attributions/{aid}` — confirm, reject, move.
pub async fn update_attribution(
    id: &str,
    aid: &str,
    patch: &pnex_core::media_review::AttributionPatch,
) -> Result<pnex_core::media_review::SpeechAttribution, ApiError> {
    let body = serde_json::to_value(patch).map_err(|e| ApiError::new(e.to_string()))?;
    client::request(
        reqwest::Method::PATCH,
        &format!(
            "/api/v1/media/{}/attributions/{}",
            urlencode(id),
            urlencode(aid)
        ),
        Some(body),
    )
    .await
}

/// `DELETE /api/v1/media/{id}/attributions/{aid}`.
pub async fn delete_attribution(id: &str, aid: &str) -> Result<Option<()>, ApiError> {
    client::request_opt(
        reqwest::Method::DELETE,
        &format!(
            "/api/v1/media/{}/attributions/{}",
            urlencode(id),
            urlencode(aid)
        ),
        None,
    )
    .await
}

/// `POST /api/v1/media/{id}/analyses` — (re)launch steps (202 + state).
pub async fn start_analyses(
    id: &str,
    input: &pnex_core::media_analysis::StartAnalyses,
) -> Result<AnalysesState, ApiError> {
    let body = serde_json::to_value(input).map_err(|e| ApiError::new(e.to_string()))?;
    client::request(
        reqwest::Method::POST,
        &format!("/api/v1/media/{}/analyses", urlencode(id)),
        Some(body),
    )
    .await
}

/// Upload parameters (metadata in the query, bytes in the body).
#[derive(Debug, Default, Clone)]
pub struct UploadParams {
    pub name: Option<String>,
    pub filename: Option<String>,
    pub kind: Option<MediaKind>,
    pub content_type: Option<String>,
    pub note: Option<String>,
}

impl UploadParams {
    fn to_query(&self) -> String {
        let mut pairs: Vec<String> = Vec::new();
        if let Some(name) = self.name.as_deref().filter(|n| !n.is_empty()) {
            pairs.push(format!("name={}", urlencode(name)));
        }
        if let Some(filename) = self.filename.as_deref().filter(|f| !f.is_empty()) {
            pairs.push(format!("filename={}", urlencode(filename)));
        }
        if let Some(kind) = self.kind {
            pairs.push(format!("kind={}", kind.as_str()));
        }
        if let Some(ct) = self.content_type.as_deref().filter(|c| !c.is_empty()) {
            pairs.push(format!("content_type={}", urlencode(ct)));
        }
        if let Some(note) = self.note.as_deref().filter(|n| !n.is_empty()) {
            pairs.push(format!("note={}", urlencode(note)));
        }
        if pairs.is_empty() {
            String::new()
        } else {
            format!("?{}", pairs.join("&"))
        }
    }
}

fn upload_path(path: &str, params: &UploadParams) -> String {
    let mut full = path.to_string();
    let q = params.to_query();
    if !q.is_empty() {
        let sep = if path.contains('?') { '&' } else { '?' };
        full.push(sep);
        full.push_str(&q[1..]);
    }
    full
}

/// `POST /api/v1/media` — upload (asset + version 1).
pub async fn upload(params: &UploadParams, bytes: Vec<u8>) -> Result<MediaAsset, ApiError> {
    client::request_upload(
        reqwest::Method::POST,
        &upload_path("/api/v1/media", params),
        bytes,
    )
    .await
}

/// `POST /api/v1/media/{id}/versions` — new version (append-only).
pub async fn add_version(
    asset_id: &str,
    params: &UploadParams,
    bytes: Vec<u8>,
) -> Result<MediaAsset, ApiError> {
    client::request_upload(
        reqwest::Method::POST,
        &upload_path(
            &format!("/api/v1/media/{}/versions", urlencode(asset_id)),
            params,
        ),
        bytes,
    )
    .await
}

/// `DELETE /api/v1/media/{id}` — purges the asset and its blobs.
pub async fn delete(id: &str) -> Result<Option<()>, ApiError> {
    client::request_opt(
        reqwest::Method::DELETE,
        &format!("/api/v1/media/{}", urlencode(id)),
        None,
    )
    .await
}

/// `DELETE /api/v1/media/{id}/versions/{n}` — purges a version and its blob
/// (409 if latest version).
pub async fn delete_version(asset_id: &str, n: i64) -> Result<Option<()>, ApiError> {
    client::request_opt(
        reqwest::Method::DELETE,
        &format!("/api/v1/media/{}/versions/{n}", urlencode(asset_id)),
        None,
    )
    .await
}

/// `POST /api/v1/media/{id}/versions/{n}/restore` — sets the current
/// version again.
pub async fn restore(asset_id: &str, n: i64) -> Result<MediaAsset, ApiError> {
    client::request(
        reqwest::Method::POST,
        &format!("/api/v1/media/{}/versions/{n}/restore", urlencode(asset_id)),
        None,
    )
    .await
}

/// API path of the current version's bytes (for `media_blob_url`).
pub fn content_path(id: &str) -> String {
    format!("/api/v1/media/{}/content", urlencode(id))
}

/// API path of version n's bytes (per-version download).
pub fn version_content_path(id: &str, n: i64) -> String {
    format!("/api/v1/media/{}/versions/{n}/content", urlencode(id))
}

/// Bytes of the current version (media download).
pub async fn content_bytes(id: &str) -> Result<Vec<u8>, ApiError> {
    client::request_bytes(reqwest::Method::GET, &content_path(id)).await
}

/// Bytes of version n (per-version download).
pub async fn version_content_bytes(asset_id: &str, n: i64) -> Result<Vec<u8>, ApiError> {
    client::request_bytes(reqwest::Method::GET, &version_content_path(asset_id, n)).await
}

/// `GET /api/v1/media/{id}/versions/diff?from=&to=` (D311).
pub async fn versions_diff(
    id: &str,
    from: i64,
    to: i64,
) -> Result<pnex_core::text_diff::TextDiff, ApiError> {
    client::request(
        reqwest::Method::GET,
        &format!("/api/v1/media/{id}/versions/diff?from={from}&to={to}"),
        None,
    )
    .await
}
