//! Client API thermo — miroir des DTOs de `pnex_coolprop::plot`
//! (pnex-core devant rester wasm-safe, les formes JSON sont dupliquées ici ;
//! les tests de round-trip des deux côtés gardent la parité).

use serde::Deserialize;

use crate::api::client;
use crate::api::error::ApiError;

/// Méta d'un axe (miroir `pnex_coolprop::AxisMeta`).
#[derive(Clone, Debug, Deserialize)]
pub struct AxisMeta {
    #[allow(dead_code)]
    pub label: String,
    pub unit: String,
    pub min: f64,
    pub max: f64,
    pub log: bool,
}

/// Courbe nommée (miroir `pnex_coolprop::DiagramCurve`).
#[derive(Clone, Debug, Deserialize)]
pub struct DiagramCurve {
    pub id: String,
    pub points: Vec<[f64; 2]>,
}

/// Résultat d'un diagramme (miroir `pnex_coolprop::DiagramResult`).
#[derive(Clone, Debug, Deserialize)]
pub struct DiagramResult {
    pub polylines: Vec<DiagramCurve>,
    pub x: AxisMeta,
    pub y: AxisMeta,
}

/// Listes du picker fluide — mélanges nommés de l'org + fluides CoolProp.
#[derive(Clone, Default, Debug, Deserialize)]
pub struct FluidsResult {
    pub mixtures: Vec<String>,
    pub fluids: Vec<String>,
    /// Mixture name → CoolProp spec (flow node freezes it at pick time).
    #[serde(default)]
    pub mixture_specs: std::collections::BTreeMap<String, String>,
}

/// `GET /api/v1/thermo/fluids` — picker fluide/mélange (plus de saisie
/// libre : l'utilisateur ne devine pas les noms).
pub async fn fluids() -> Result<FluidsResult, ApiError> {
    client::request(reqwest::Method::GET, "/api/v1/thermo/fluids", None).await
}

/// `POST /api/v1/thermo/diagram` — polylines (cache serveur).
pub async fn diagram(
    fluid: &str,
    diagram: &str,
    isolines: Vec<pnex_core::ThermoIsoline>,
    pressure: Option<f64>,
) -> Result<DiagramResult, ApiError> {
    client::request(
        reqwest::Method::POST,
        "/api/v1/thermo/diagram",
        Some(serde_json::json!({
            "fluid": fluid,
            "diagram": diagram,
            "isolines": isolines,
            "pressure": pressure,
        })),
    )
    .await
}

/// `POST /api/v1/thermo/cycle-points` — états → propriétés + coordonnées.
pub async fn cycle_points(
    fluid: &str,
    diagram: &str,
    points: Vec<serde_json::Value>,
    pressure: Option<f64>,
) -> Result<Vec<CyclePoint>, ApiError> {
    client::request(
        reqwest::Method::POST,
        "/api/v1/thermo/cycle-points",
        Some(serde_json::json!({
            "fluid": fluid,
            "diagram": diagram,
            "points": points,
            "pressure": pressure,
        })),
    )
    .await
}

/// Point de cycle résolu (miroir `pnex_coolprop::CyclePointOutput`).
#[derive(Clone, Debug, Deserialize)]
pub struct CyclePoint {
    pub label: String,
    #[allow(dead_code)]
    pub props: std::collections::BTreeMap<String, f64>,
    pub coords: [f64; 2],
    #[allow(dead_code)]
    pub phase: Option<String>,
}
