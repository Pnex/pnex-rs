//! Endpoints pins Brick 0 — `GET /api/v1/devices/{id}/pins`,
//! `POST /api/v1/devices/{id}/commands` (action manuelle D17),
//! `POST /api/v1/devices/{id}/config-sector` (secteur PNEXCFG1 4 Ko).

use crate::api::client;
use crate::api::error::ApiError;

/// Un pin du device générique (miroir du PinDto backend).
#[derive(Debug, Clone, PartialEq, serde::Deserialize)]
pub struct PinInfo {
    pub gpio: i32,
    pub label: String,
    pub mode: String,
    pub role: String,
    pub pullup: bool,
    pub safe_state: String,
    pub enabled: bool,
    #[serde(default)]
    pub last_value: Option<serde_json::Value>,
}

/// Réponse `GET /devices/{id}/pins`.
#[derive(Debug, Clone, PartialEq, serde::Deserialize)]
pub struct PinsResponse {
    pub pins: Vec<PinInfo>,
    pub connected: bool,
}

/// Corps `POST /devices/{id}/commands` (op set_mode | write | subscribe).
#[derive(Debug, Clone)]
pub struct Command {
    pub op: &'static str,
    pub gpio: u16,
    pub mode: Option<&'static str>,
    pub safe_state: Option<&'static str>,
    pub value: Option<serde_json::Value>,
    pub interval_ms: Option<u32>,
}

impl Command {
    fn to_json(&self) -> serde_json::Value {
        serde_json::json!({
            "op": self.op,
            "gpio": self.gpio,
            "mode": self.mode,
            "opts": self.safe_state.map(|s| serde_json::json!({"safe_state": s})),
            "value": self.value,
            "interval_ms": self.interval_ms
        })
    }
}

/// Corps `POST /config-sector` (chaînes claires — le secteur PNEXCFG1 fait
/// le reste).
#[derive(Debug, Clone, serde::Serialize)]
pub struct ConfigSectorRequest {
    pub wifi_ssid: String,
    pub wifi_password: String,
    pub host: String,
    pub ws_ssl: bool,
}

/// `GET /devices/{id}/pins`.
pub async fn pins(device_pk: i64) -> Result<PinsResponse, ApiError> {
    client::request(
        reqwest::Method::GET,
        &format!("/api/v1/devices/{device_pk}/pins"),
        None,
    )
    .await
}

/// `POST /devices/{id}/commands` — 400 si illégal (raison chip-caps
/// relayée telle quelle), 409 si le device est offline.
pub async fn command(device_pk: i64, cmd: Command) -> Result<(), ApiError> {
    client::request_opt::<serde_json::Value>(
        reqwest::Method::POST,
        &format!("/api/v1/devices/{device_pk}/commands"),
        Some(cmd.to_json()),
    )
    .await?
    .ok_or_else(|| ApiError {
        message: "réponse vide inattendue".into(),
    })
    .map(|_| ())
}

/// `POST /config-sector` — les 4096 octets du secteur PNEXCFG1 à flasher
/// (token device inclus côté serveur, jamais au client).
pub async fn config_sector(device_pk: i64, req: &ConfigSectorRequest) -> Result<Vec<u8>, ApiError> {
    client::request_bytes_with_body(
        reqwest::Method::POST,
        &format!("/api/v1/devices/{device_pk}/config-sector"),
        serde_json::to_value(req).unwrap_or_default(),
    )
    .await
}
