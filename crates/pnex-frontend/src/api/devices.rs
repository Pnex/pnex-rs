//! Device endpoints — org-scoped registry + global catalogue (Phase 4).
//!
//! `create` returns raw JSON: 201 → `Device`, or 200 `{"reactivated": true}`
//! (reactivation of a known inactive device) — the caller tells them apart
//! by the presence of `reactivated`.

use pnex_core::{CreateDevice, Device, DeviceCapability, Paginated, PredefinedDevice};

use crate::api::client;
use crate::api::error::ApiError;

/// Filters of `GET /api/v1/devices` + pagination (D14) — absent = server
/// defaults (limit 10, offset 0).
#[derive(Default)]
pub struct DeviceFilters {
    /// Type name ("all" on the server = no-op, here we send nothing).
    pub device_type: Option<String>,
    pub capability: Option<String>,
    /// Correspondance exacte.
    pub device_id: Option<String>,
    /// Multi-field OR search (device_id, model, type, capabilities).
    pub search: Option<String>,
    pub active: Option<bool>,
    /// D42 effective label filter (`name` or `name:value`).
    pub label: Option<String>,
    pub limit: Option<i64>,
    pub offset: Option<i64>,
}

impl DeviceFilters {
    fn to_query(&self) -> String {
        let mut parts: Vec<String> = Vec::new();
        if let Some(v) = &self.device_type {
            parts.push(format!("device_type={}", urlencode(v)));
        }
        if let Some(v) = &self.capability {
            parts.push(format!("capability={}", urlencode(v)));
        }
        if let Some(v) = &self.device_id {
            parts.push(format!("device_id={}", urlencode(v)));
        }
        if let Some(v) = &self.search {
            parts.push(format!("search={}", urlencode(v)));
        }
        if let Some(v) = self.active {
            parts.push(format!("active={v}"));
        }
        if let Some(v) = self.label.as_deref().filter(|v| !v.is_empty()) {
            parts.push(format!("label={}", urlencode(v)));
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

/// Minimal percent encoding (the front has no url crate).
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

/// `GET /api/v1/devices` — devices of the current org, paginated envelope.
pub async fn list(filters: &DeviceFilters) -> Result<Paginated<Device>, ApiError> {
    client::request(
        reqwest::Method::GET,
        &format!("/api/v1/devices{}", filters.to_query()),
        None,
    )
    .await
}

/// `GET /api/v1/devices`, every page (device pickers); the filters' own
/// `limit`/`offset` must be left unset.
pub async fn list_all(filters: &DeviceFilters) -> Result<Vec<Device>, ApiError> {
    client::request_all(&format!("/api/v1/devices{}", filters.to_query())).await
}

/// `POST /api/v1/devices` — creation (201) or reactivation (200).
pub async fn create(params: CreateDevice) -> Result<serde_json::Value, ApiError> {
    client::request(
        reqwest::Method::POST,
        "/api/v1/devices",
        Some(serde_json::to_value(params).unwrap_or_default()),
    )
    .await
}

/// `GET /api/v1/devices/{id}` — detail.
pub async fn detail(id: i64) -> Result<Device, ApiError> {
    client::request(reqwest::Method::GET, &format!("/api/v1/devices/{id}"), None).await
}

/// `PUT /api/v1/devices/{id}/board` — another board variant of the same
/// chip (O39); takes effect at the next build.
pub async fn set_board(id: i64, board_id: i64) -> Result<serde_json::Value, ApiError> {
    client::request(
        reqwest::Method::PUT,
        &format!("/api/v1/devices/{id}/board"),
        Some(serde_json::json!({ "board_id": board_id })),
    )
    .await
}

/// `DELETE /api/v1/devices/{id}` — device + token + build records.
pub async fn delete(id: i64) -> Result<(), ApiError> {
    client::request_opt::<serde_json::Value>(
        reqwest::Method::DELETE,
        &format!("/api/v1/devices/{id}"),
        None,
    )
    .await
    .map(|_| ())
}

/// `GET /api/v1/device-capabilities`, every page (pickers).
pub async fn capabilities() -> Result<Vec<DeviceCapability>, ApiError> {
    client::request_all("/api/v1/device-capabilities").await
}

/// `GET /api/v1/predefined-devices`, every page (registration form
/// selects; the catalogue page paginates on its own).
pub async fn predefined_devices() -> Result<Vec<PredefinedDevice>, ApiError> {
    client::request_all("/api/v1/predefined-devices").await
}

/// Filters of the Catalogue page — search + type + board, pushed to the
/// server (D14: the grid is paginated by the API).
#[derive(Default)]
pub struct CatalogFilters {
    pub search: Option<String>,
    pub device_type: Option<String>,
    pub board: Option<String>,
    pub limit: i64,
    pub offset: i64,
}

/// `GET /api/v1/predefined-devices` — catalogue page with filters.
pub async fn predefined_devices_page(
    filters: &CatalogFilters,
) -> Result<Paginated<PredefinedDevice>, ApiError> {
    let mut parts: Vec<String> = vec![format!("limit={}", filters.limit)];
    if let Some(v) = &filters.search {
        parts.push(format!("search={}", urlencode(v)));
    }
    if let Some(v) = &filters.device_type {
        parts.push(format!("device_type={}", urlencode(v)));
    }
    if let Some(v) = &filters.board {
        parts.push(format!("board={}", urlencode(v)));
    }
    parts.push(format!("offset={}", filters.offset));
    client::request(
        reqwest::Method::GET,
        &format!("/api/v1/predefined-devices?{}", parts.join("&")),
        None,
    )
    .await
}
