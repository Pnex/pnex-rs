//! API média (D21) — client du contrôleur `/api/v1/media` (contrat
//! `docs/contracts/media.http`). Upload octet-stream via
//! `client::request_upload` (pas de multipart), listing paginé D14.

use serde::{Deserialize, Serialize};

use crate::api::client;
use crate::api::error::ApiError;

/// Kind d'asset média — miroir du contrôleur (`photo | panorama | splat`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MediaKind {
    Photo,
    Panorama,
    Splat,
    /// Plan d'étage — consommé par la base map (viz) et référencé par
    /// `tour_versions.doc` (S9 studio.md) ; kind = string applicatif.
    Floorplan,
    /// ONNX model backing a vision registry entry (camera-video.md D81).
    Model,
}

impl MediaKind {
    pub fn as_str(self) -> &'static str {
        match self {
            MediaKind::Photo => "photo",
            MediaKind::Panorama => "panorama",
            MediaKind::Splat => "splat",
            MediaKind::Floorplan => "floorplan",
            MediaKind::Model => "model",
        }
    }
}

/// Version d'un asset média.
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

/// Asset média (détail = même forme que la réponse d'upload 201).
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct MediaAsset {
    pub id: String,
    pub kind: String,
    pub name: String,
    pub description: Option<String>,
    pub metadata: Option<serde_json::Value>,
    /// Labels propres (D42) — effectifs via la couche transverse.
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
    /// Kind typé (défaut photo si valeur serveur inconnue).
    pub fn media_kind(&self) -> MediaKind {
        match self.kind.as_str() {
            "panorama" => MediaKind::Panorama,
            "splat" => MediaKind::Splat,
            "floorplan" => MediaKind::Floorplan,
            "model" => MediaKind::Model,
            _ => MediaKind::Photo,
        }
    }
}

/// Asset résumé (liste = même forme).
pub type MediaAssetSummary = MediaAsset;

/// Enveloppe paginée D14 (next/previous consommés par l'UI V2 — Pager
/// n'utilise que count).
#[derive(Clone, Debug, Deserialize)]
#[allow(dead_code)]
pub struct MediaPage {
    pub count: i64,
    pub next: Option<String>,
    pub previous: Option<String>,
    pub results: Vec<MediaAssetSummary>,
}

/// Filtres de liste (D14) — encodés en query string maison (école flows.rs).
#[derive(Debug, Default, Clone)]
pub struct MediaFilters {
    /// Kinds souhaités (vide = tous) — encodés `kind=a,b` côté serveur.
    pub kinds: Vec<MediaKind>,
    pub search: Option<String>,
    /// Filtre label effectif `name` ou `name:valeur` (D42).
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

/// Percent-encoding minimal (le front n'a pas la crate url — école flows.rs).
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

/// `GET /api/v1/media` — liste paginée.
pub async fn list(filters: &MediaFilters) -> Result<MediaPage, ApiError> {
    client::request(
        reqwest::Method::GET,
        &format!("/api/v1/media{}", filters.to_query()),
        None,
    )
    .await
}

/// `GET /api/v1/media/{id}` — détail + versions.
pub async fn detail(id: &str) -> Result<MediaAsset, ApiError> {
    client::request(reqwest::Method::GET, &format!("/api/v1/media/{id}"), None).await
}

/// `GET /api/v1/media/{id}/versions` — liste des versions.
pub async fn versions(id: &str) -> Result<Vec<MediaVersion>, ApiError> {
    #[derive(serde::Deserialize)]
    struct Page {
        results: Vec<MediaVersion>,
    }

    let page: Page = client::request(
        reqwest::Method::GET,
        &format!("/api/v1/media/{id}/versions"),
        None,
    )
    .await?;
    Ok(page.results)
}

/// Paramètres d'upload (métadonnées en query, octets en corps).
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

/// `POST /api/v1/media/{id}/versions` — nouvelle version (append-only).
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

/// `PATCH /api/v1/media/{id}` — rename/description (UI V2 ; l'API client
/// est posée complète d'un bloc, école icons.rs).
#[allow(dead_code)]
pub async fn rename(
    id: &str,
    name: Option<String>,
    description: Option<String>,
) -> Result<MediaAsset, ApiError> {
    let mut body = serde_json::Map::new();
    if let Some(name) = name {
        body.insert("name".into(), serde_json::json!(name));
    }
    if let Some(description) = description {
        body.insert("description".into(), serde_json::json!(description));
    }
    client::request(
        reqwest::Method::PATCH,
        &format!("/api/v1/media/{}", urlencode(id)),
        Some(serde_json::Value::Object(body)),
    )
    .await
}

/// `DELETE /api/v1/media/{id}` — purge l'asset et ses blobs.
pub async fn delete(id: &str) -> Result<Option<()>, ApiError> {
    client::request_opt(
        reqwest::Method::DELETE,
        &format!("/api/v1/media/{}", urlencode(id)),
        None,
    )
    .await
}

/// `DELETE /api/v1/media/{id}/versions/{n}` — purge une version et son blob
/// (409 si dernière version).
pub async fn delete_version(asset_id: &str, n: i64) -> Result<Option<()>, ApiError> {
    client::request_opt(
        reqwest::Method::DELETE,
        &format!("/api/v1/media/{}/versions/{n}", urlencode(asset_id)),
        None,
    )
    .await
}

/// `POST /api/v1/media/{id}/versions/{n}/restore` — re-positionne la
/// version courante.
pub async fn restore(asset_id: &str, n: i64) -> Result<MediaAsset, ApiError> {
    client::request(
        reqwest::Method::POST,
        &format!("/api/v1/media/{}/versions/{n}/restore", urlencode(asset_id)),
        None,
    )
    .await
}

/// Chemin d'API des octets de la version courante (pour `media_blob_url`).
pub fn content_path(id: &str) -> String {
    format!("/api/v1/media/{}/content", urlencode(id))
}

/// Chemin d'API des octets de la version n (téléchargement par version).
pub fn version_content_path(id: &str, n: i64) -> String {
    format!("/api/v1/media/{}/versions/{n}/content", urlencode(id))
}

/// Octets de la version courante (téléchargement média).
pub async fn content_bytes(id: &str) -> Result<Vec<u8>, ApiError> {
    client::request_bytes(reqwest::Method::GET, &content_path(id)).await
}

/// Octets de la version n (téléchargement par version).
pub async fn version_content_bytes(asset_id: &str, n: i64) -> Result<Vec<u8>, ApiError> {
    client::request_bytes(reqwest::Method::GET, &version_content_path(asset_id, n)).await
}
