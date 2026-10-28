//! `GET /api/v1/boards` — v2 board profiles (wizard variant picker).

use crate::api::client;
use crate::api::error::ApiError;

/// One pin of a board profile (mirror of the backend BoardPinDto).
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

/// A board variant of the catalogue.
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
    /// Screens declared in the profile (built-in peripherals).
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
