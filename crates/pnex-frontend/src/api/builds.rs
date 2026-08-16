//! Endpoints builds firmware (Phase 6) — `POST /build-firmware`,
//! `GET /build-records` (paginé D14), `DELETE /build-records/{id}`,
//! `GET /download/firmware/{device_id}` (octets proxifiés).

use pnex_core::{BuildRecord, CreateBuild, CreateBuildResponse, Paginated};

use crate::api::client;
use crate::api::error::ApiError;

/// Filtres de `GET /api/v1/build-records` + pagination.
#[derive(Default)]
pub struct BuildFilters {
    /// Correspondance exacte sur l'identifiant firmware.
    pub device_id: Option<String>,
    pub success: Option<bool>,
    pub limit: Option<i64>,
    pub offset: Option<i64>,
}

impl BuildFilters {
    fn to_query(&self) -> String {
        let mut parts: Vec<String> = Vec::new();
        if let Some(v) = &self.device_id {
            parts.push(format!("device_id={}", urlencode(v)));
        }
        if let Some(v) = self.success {
            parts.push(format!("success={v}"));
        }
        if let Some(v) = self.limit {
            parts.push(format!("limit={v}"));
        }
        if let Some(v) = self.offset {
            parts.push(format!("offset={v}"));
        }
        if parts.is_empty() {
            String::new()
        } else {
            format!("?{}", parts.join("&"))
        }
    }
}

fn urlencode(value: &str) -> String {
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

/// `GET /api/v1/build-records` — records de l'org courante.
pub async fn list(filters: &BuildFilters) -> Result<Paginated<BuildRecord>, ApiError> {
    client::request(
        reqwest::Method::GET,
        &format!("/api/v1/build-records{}", filters.to_query()),
        None,
    )
    .await
}

/// `POST /api/v1/build-firmware` — enregistre le job de build.
pub async fn create(params: CreateBuild) -> Result<CreateBuildResponse, ApiError> {
    client::request(
        reqwest::Method::POST,
        "/api/v1/build-firmware",
        Some(serde_json::to_value(params).unwrap_or_default()),
    )
    .await
}

/// `DELETE /api/v1/build-records/{id}` — 204 attendu.
pub async fn delete(id: i64) -> Result<(), ApiError> {
    client::request_opt::<serde_json::Value>(
        reqwest::Method::DELETE,
        &format!("/api/v1/build-records/{id}"),
        None,
    )
    .await
    .map(|_| ())
}

/// `GET /api/v1/download/firmware/{device_id}` — octets du binaire (proxy
/// serveur). L'appelant déclenche le téléchargement navigateur
/// (`util::save_blob`).
pub async fn download(device_id: &str) -> Result<Vec<u8>, ApiError> {
    client::request_bytes(
        reqwest::Method::GET,
        &format!("/api/v1/download/firmware/{}", urlencode(device_id)),
    )
    .await
}
