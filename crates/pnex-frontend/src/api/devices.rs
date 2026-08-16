//! Endpoints devices — registre scopé org + catalogue global (Phase 4).
//!
//! `create` renvoie un JSON brut : 201 → `Device`, ou 200 `{"detail": …}`
//! (réactivation d'un device inactif connu) — l'appelant distingue sur la
//! présence de `detail`.

use pnex_core::{CreateDevice, Device, DeviceCapability, PredefinedDevice};

use crate::api::client;
use crate::api::error::ApiError;

/// Filtres de `GET /api/v1/devices` (parité query Django) — absents = tous.
#[derive(Default)]
pub struct DeviceFilters {
    /// Nom de type (« all » côté serveur = no-op, ici on n'envoie rien).
    pub device_type: Option<String>,
    pub capability: Option<String>,
    /// Correspondance exacte.
    pub device_id: Option<String>,
    pub active: Option<bool>,
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
        if let Some(v) = self.active {
            parts.push(format!("active={v}"));
        }
        if parts.is_empty() {
            String::new()
        } else {
            format!("?{}", parts.join("&"))
        }
    }
}

/// Encodage percent minimal (le front n'a pas la crate url).
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

/// `GET /api/v1/devices` — devices de l'org courante, non paginé.
pub async fn list(filters: &DeviceFilters) -> Result<Vec<Device>, ApiError> {
    client::request(
        reqwest::Method::GET,
        &format!("/api/v1/devices{}", filters.to_query()),
        None,
    )
    .await
}

/// `POST /api/v1/devices` — création (201) ou réactivation (200).
pub async fn create(params: CreateDevice) -> Result<serde_json::Value, ApiError> {
    client::request(
        reqwest::Method::POST,
        "/api/v1/devices",
        Some(serde_json::to_value(params).unwrap_or_default()),
    )
    .await
}

/// `GET /api/v1/devices/{id}` — détail.
pub async fn detail(id: i64) -> Result<Device, ApiError> {
    client::request(reqwest::Method::GET, &format!("/api/v1/devices/{id}"), None).await
}

/// `PATCH /api/v1/devices/{id}` — metadata uniquement (contrat Django).
pub async fn update_metadata(id: i64, metadata: serde_json::Value) -> Result<Device, ApiError> {
    client::request(
        reqwest::Method::PATCH,
        &format!("/api/v1/devices/{id}"),
        Some(serde_json::json!({ "metadata": metadata })),
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

/// `GET /api/v1/device-capabilities` — catalogue global.
pub async fn capabilities() -> Result<Vec<DeviceCapability>, ApiError> {
    client::request(reqwest::Method::GET, "/api/v1/device-capabilities", None).await
}

/// `GET /api/v1/predefined-devices` — catalogue global (non filtré : la page
/// crée depuis la liste complète).
pub async fn predefined_devices() -> Result<Vec<PredefinedDevice>, ApiError> {
    client::request(reqwest::Method::GET, "/api/v1/predefined-devices", None).await
}
