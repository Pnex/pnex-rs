//! Thermo API client — mirror of the `pnex_coolprop::plot` DTOs
//! (pnex-core must stay wasm-safe, so the JSON shapes are duplicated here;
//! the round-trip tests on both sides keep them in parity).

use serde::Deserialize;

use crate::api::client;
use crate::api::error::ApiError;

/// Axis metadata (mirror of `pnex_coolprop::AxisMeta`).
#[derive(Clone, Debug, Deserialize)]
pub struct AxisMeta {
    #[allow(dead_code)]
    pub label: String,
    pub unit: String,
    pub min: f64,
    pub max: f64,
    pub log: bool,
}

/// Named curve (mirror of `pnex_coolprop::DiagramCurve`).
#[derive(Clone, Debug, Deserialize)]
pub struct DiagramCurve {
    pub id: String,
    pub points: Vec<[f64; 2]>,
}

/// Diagram result (mirror of `pnex_coolprop::DiagramResult`).
#[derive(Clone, Debug, Deserialize)]
pub struct DiagramResult {
    pub polylines: Vec<DiagramCurve>,
    pub x: AxisMeta,
    pub y: AxisMeta,
}

/// Fluid picker lists — the org's named mixtures + CoolProp fluids.
#[derive(Clone, Default, Debug, Deserialize)]
pub struct FluidsResult {
    pub mixtures: Vec<String>,
    pub fluids: Vec<String>,
    /// Mixture name → CoolProp spec (flow node freezes it at pick time).
    #[serde(default)]
    pub mixture_specs: std::collections::BTreeMap<String, String>,
}

/// `GET /api/v1/thermo/fluids` — fluid/mixture picker (no more free
/// input: users do not guess the names).
pub async fn fluids() -> Result<FluidsResult, ApiError> {
    client::request(reqwest::Method::GET, "/api/v1/thermo/fluids", None).await
}

/// `POST /api/v1/thermo/diagram/query`: polylines (server cache).
pub async fn diagram(
    fluid: &str,
    diagram: &str,
    isolines: Vec<pnex_core::ThermoIsoline>,
    pressure: Option<f64>,
) -> Result<DiagramResult, ApiError> {
    client::request(
        reqwest::Method::POST,
        "/api/v1/thermo/diagram/query",
        Some(serde_json::json!({
            "fluid": fluid,
            "diagram": diagram,
            "isolines": isolines,
            "pressure": pressure,
        })),
    )
    .await
}

/// `POST /api/v1/thermo/cycle-points/query`: states to properties +
/// coordinates.
pub async fn cycle_points(
    fluid: &str,
    diagram: &str,
    points: Vec<serde_json::Value>,
    pressure: Option<f64>,
) -> Result<Vec<CyclePoint>, ApiError> {
    #[derive(Deserialize)]
    struct Points {
        points: Vec<CyclePoint>,
    }
    let answer: Points = client::request(
        reqwest::Method::POST,
        "/api/v1/thermo/cycle-points/query",
        Some(serde_json::json!({
            "fluid": fluid,
            "diagram": diagram,
            "points": points,
            "pressure": pressure,
        })),
    )
    .await?;
    Ok(answer.points)
}

/// Resolved cycle point (mirror of `pnex_coolprop::CyclePointOutput`).
#[derive(Clone, Debug, Deserialize)]
pub struct CyclePoint {
    pub label: String,
    #[allow(dead_code)]
    pub props: std::collections::BTreeMap<String, f64>,
    pub coords: [f64; 2],
    #[allow(dead_code)]
    pub phase: Option<String>,
}
