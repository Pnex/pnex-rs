//! Endpoints télémétrie (page Visualisation, 2026-08-19) — catalogue
//! des séries disponibles et points d'une série sur une fenêtre preset.

use pnex_core::{TelemetryCatalog, TelemetrySeriesResponse};

use crate::api::client;
use crate::api::error::ApiError;

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

/// `GET /api/v1/telemetry/catalog` — séries (métrique × device) de
/// l'org courante, dégradé `available: false` sans O2.
pub async fn catalog() -> Result<TelemetryCatalog, ApiError> {
    client::request(reqwest::Method::GET, "/api/v1/telemetry/catalog", None).await
}

/// `GET /api/v1/telemetry/series` — points d'UNE série ; `window` ∈
/// 1h/6h/24h (validated côté serveur, charset fermé anti-injection).
pub async fn series(
    metric: &str,
    device_id: &str,
    window: &str,
) -> Result<TelemetrySeriesResponse, ApiError> {
    client::request(
        reqwest::Method::GET,
        &format!(
            "/api/v1/telemetry/series?metric={}&device_id={}&window={window}",
            urlencode(metric),
            urlencode(device_id)
        ),
        None,
    )
    .await
}
