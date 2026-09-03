//! Endpoints pins Brick 0 — `GET /api/v1/devices/{id}/pins` et
//! `POST /api/v1/devices/{id}/commands` (action manuelle D17).

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
    /// Cadence de lecture persistée (ms, 0/absent = manuel) — initialise
    /// le select de cadence à sa valeur effective (leçon des selects
    /// contrôlés : le select doit AFFICHER l'état réel, pas un défaut).
    #[serde(default)]
    pub interval_ms: Option<u32>,
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
