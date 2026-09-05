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

/// Un pin du pinout (`source` = instance | overlay — le défaut de la carte
/// quand le device n'a jamais été connecté).
#[derive(Clone, Debug)]
pub struct PinoutPin {
    pub gpio: i32,
    pub label: String,
    pub mode: String,
    /// `instance` (mode réel) ou `overlay` (défaut carte, device jamais
    /// connecté) — consommé par l'inspecteur (suffixe du label du pin).
    pub source: String,
}

/// Réponse `GET /devices/{id}/pinout` — pins + connexion WS du device.
#[derive(Debug, Clone)]
pub struct Pinout {
    /// Le device générique est-il connecté au serveur ? `false` = hors
    /// ligne (pas encore reconnecté après un restart, par exemple).
    pub connected: bool,
    pub pins: Vec<PinoutPin>,
}

/// `GET /devices/{id}/pinout` — pinout complet (instances + overlay).
pub async fn pinout(device_pk: i64) -> Result<Pinout, ApiError> {
    let body = client::request::<serde_json::Value>(
        reqwest::Method::GET,
        &format!("/api/v1/devices/{device_pk}/pinout"),
        None,
    )
    .await?;
    let mut pins = Vec::new();
    if let Some(list) = body["pins"].as_array() {
        for p in list {
            pins.push(PinoutPin {
                gpio: p["gpio"].as_i64().unwrap_or_default() as i32,
                label: p["label"].as_str().unwrap_or_default().to_string(),
                mode: p["mode"].as_str().unwrap_or_default().to_string(),
                source: p["source"].as_str().unwrap_or_default().to_string(),
            });
        }
    }
    Ok(Pinout {
        connected: body["connected"].as_bool().unwrap_or(false),
        pins,
    })
}

/// `POST /devices/{id}/commands` — 400 si illégal (raison chip-caps
/// relayée telle quelle), 409 si le device est offline. Le corps de réponse
/// est retourné tel quel (contient `flow_impacts` quand un set_mode a arrêté
/// des flows déployés — Phase 6).
pub async fn command(device_pk: i64, cmd: Command) -> Result<serde_json::Value, ApiError> {
    client::request_opt::<serde_json::Value>(
        reqwest::Method::POST,
        &format!("/api/v1/devices/{device_pk}/commands"),
        Some(cmd.to_json()),
    )
    .await?
    .ok_or_else(|| ApiError::new("réponse vide inattendue"))
}
