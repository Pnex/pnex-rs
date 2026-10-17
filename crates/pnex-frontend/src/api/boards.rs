//! `GET /api/v1/boards` — profils de board v2 (picker de variantes wizard).

use crate::api::client;
use crate::api::error::ApiError;

/// Un pin d'un profil board (miroir du BoardPinDto backend).
#[derive(Debug, Clone, PartialEq, serde::Deserialize)]
pub struct BoardPinDto {
    pub label: String,
    pub gpio: Option<i32>,
    pub kind: String,
    pub pos: serde_json::Value,
    #[serde(default)]
    pub note: Option<String>,
    #[serde(rename = "fn", default)]
    pub fns: Vec<String>,
    #[serde(default)]
    pub flags: Vec<String>,
    #[serde(default)]
    pub available_modes: Vec<String>,
    #[serde(default)]
    pub warnings: Vec<String>,
}

/// Une variante de board du catalogue.
#[derive(Debug, Clone, PartialEq, serde::Deserialize)]
pub struct Board {
    pub id: i64,
    pub name: String,
    pub soc: String,
    #[serde(default)]
    pub pretty_name: Option<String>,
    #[serde(default)]
    pub chip_label: Option<String>,
    #[serde(default)]
    pub profile_id: Option<String>,
    #[serde(default)]
    pub pio_board: Option<String>,
    pub layout: serde_json::Value,
    #[serde(default)]
    pub peripherals: serde_json::Value,
    #[serde(default)]
    pub pins: Vec<BoardPinDto>,
}

impl Board {
    /// Écrans déclarés dans le profil (périphériques intégrés).
    pub fn screens(&self) -> Vec<serde_json::Value> {
        self.peripherals
            .get("screens")
            .and_then(|s| s.as_array())
            .cloned()
            .unwrap_or_default()
    }
}

/// `GET /api/v1/boards`, every page — `?soc=` filters by family (wizard
/// picker).
pub async fn list(soc: Option<&str>) -> Result<Vec<Board>, ApiError> {
    let q = soc.map(|s| format!("?soc={s}")).unwrap_or_default();
    client::request_all(&format!("/api/v1/boards{q}")).await
}
