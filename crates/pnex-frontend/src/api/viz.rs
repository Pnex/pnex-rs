//! Client API de la carte POI-first (D35–D39) : POI, clustering backend
//! (D37), liens typés (D39), positions GPS devices (D38). Miroir de
//! `controllers/pois.rs` (contrat : `docs/contracts/viz.http`).

use serde::{Deserialize, Serialize};

use crate::api::client;
use crate::api::error::ApiError;
use crate::api::media::urlencode;

pub const MAP_STYLE_URL: &str = "https://map.alpine-box.com/style/light-en";

// ─────────────────────────── DTO ───────────────────────────

/// Arête POI→cible (D42 : `resource_edges`, `viz_links` absorbé) —
/// l'id est un entier, le `label` vit dans `placement.label`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct VizLink {
    pub id: i64,
    pub source_kind: String,
    pub source_id: String,
    pub target_kind: String,
    pub target_id: String,
    pub placement: Option<serde_json::Value>,
    /// Nom résolu côté serveur (batch org) — `None` si cible morte ou
    /// hydratation en échec.
    #[serde(default)]
    pub target_label: Option<String>,
    /// Cible disparue des tables (supprimée hors cascade) — ligne
    /// désactivée côté UI (D26 : tolérée au rendu, jamais panic).
    #[serde(default)]
    pub target_dead: bool,
}

/// Placement d'un device sur un POI (D43) — plusieurs devices par repère,
/// mais un device = **un seul** placement à la fois (objet physique).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DevicePlacement {
    pub id: i64,
    pub device_id: String,
    /// Localisation fine dans le site (« Rack 3 — Allée B »).
    pub location_detail: Option<String>,
}

/// Le POI (objet primaire de la carte — D35).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Poi {
    pub id: String,
    pub mode: String,
    pub latitude: Option<f64>,
    pub longitude: Option<f64>,
    /// Devices placés sur ce POI (D43 — remplace la colonne `device_id`).
    #[serde(default)]
    pub devices: Vec<DevicePlacement>,
    pub label: String,
    pub emoji: String,
    pub location_detail: Option<String>,
    /// Aperçu épinglé (000020) : kind ∈ media_asset | dashboard | tour.
    #[serde(default)]
    pub preview_kind: Option<String>,
    #[serde(default)]
    pub preview_id: Option<String>,
    pub metadata: Option<serde_json::Value>,
    pub links: Vec<VizLink>,
    pub created_at: String,
    pub updated_at: String,
}

/// Position GPS live d'un device (D38 — dernière connue, source
/// `telemetry` ou `manual`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DevicePosition {
    pub device_id: String,
    pub latitude: f64,
    pub longitude: f64,
    pub altitude_m: Option<f64>,
    pub accuracy_m: Option<f64>,
    pub speed_mps: Option<f64>,
    pub heading_deg: Option<f64>,
    pub source: String,
    pub positioned_at: String,
}

/// Item de la réponse cluster : point individuel (`count == 1`, `id`
/// renseigné) ou cluster numéroté (D37).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ClusterItem {
    pub lat: f64,
    pub lon: f64,
    pub count: u32,
    pub id: Option<String>,
    pub label: String,
    pub emoji: String,
}

#[derive(Clone, Debug, Deserialize)]
pub struct ClusterResponse {
    #[allow(dead_code)]
    pub zoom: i32,
    #[allow(dead_code)]
    pub total: usize,
    pub items: Vec<ClusterItem>,
}

#[derive(Clone, Debug, Default)]
pub struct PoiFilters {
    pub search: Option<String>,
    pub emoji: Option<String>,
    pub has_device: bool,
    pub has_position: bool,
    pub limit: Option<i64>,
    pub offset: Option<i64>,
}

impl PoiFilters {
    /// Paires de filtres communes liste / cluster (bbox/zoom/pagination en
    /// plus selon l'endpoint).
    fn filter_pairs(&self) -> Vec<String> {
        let mut pairs: Vec<String> = Vec::new();
        if let Some(search) = self.search.as_deref().filter(|s| !s.is_empty()) {
            pairs.push(format!("search={}", urlencode(search)));
        }
        if let Some(emoji) = self.emoji.as_deref().filter(|e| !e.is_empty()) {
            pairs.push(format!("emoji={}", urlencode(emoji)));
        }
        if self.has_device {
            pairs.push("has_device=true".into());
        }
        if self.has_position {
            pairs.push("has_position=true".into());
        }
        pairs
    }

    fn to_query(&self) -> String {
        let mut pairs = self.filter_pairs();
        if let Some(limit) = self.limit {
            pairs.push(format!("limit={limit}"));
        }
        if let Some(offset) = self.offset {
            pairs.push(format!("offset={offset}"));
        }
        if pairs.is_empty() {
            String::new()
        } else {
            format!("?{}", pairs.join("&"))
        }
    }
}

#[derive(Clone, Debug, Deserialize)]
pub struct PoiPage {
    pub count: i64,
    #[allow(dead_code)]
    pub next: Option<String>,
    #[allow(dead_code)]
    pub previous: Option<String>,
    pub results: Vec<Poi>,
}

// ─────────────────────────── POI ───────────────────────────

/// `GET /api/v1/pois` — liste globale org (D14).
pub async fn list_pois(filters: &PoiFilters) -> Result<PoiPage, ApiError> {
    client::request(
        reqwest::Method::GET,
        &format!("/api/v1/pois{}", filters.to_query()),
        None,
    )
    .await
}

/// `POST /api/v1/pois` — création (label + lat/lon requis).
pub async fn create_poi(body: serde_json::Value) -> Result<Poi, ApiError> {
    client::request(reqwest::Method::POST, "/api/v1/pois", Some(body)).await
}

/// `GET /api/v1/pois/{id}`.
pub async fn poi_detail(id: &str) -> Result<Poi, ApiError> {
    client::request(reqwest::Method::GET, &format!("/api/v1/pois/{id}"), None).await
}

/// `PATCH /api/v1/pois/{id}` — édition/déplacement (`null` efface : emoji,
/// location_detail ; 1 PATCH au pointer-up, D34).
pub async fn update_poi(id: &str, body: serde_json::Value) -> Result<Poi, ApiError> {
    client::request(
        reqwest::Method::PATCH,
        &format!("/api/v1/pois/{id}"),
        Some(body),
    )
    .await
}

/// `POST /api/v1/pois/{id}/devices` — attache un device (D43, délibéré).
/// 409 = device déjà placé : `ApiError.body` porte alors le placement
/// courant (`current_placement_id`, `current_pin_id`, `current_pin_label`)
/// pour proposer le déplacement en connaissance de cause.
pub async fn attach_poi_device(
    pin_id: &str,
    device_id: &str,
    location_detail: Option<String>,
) -> Result<DevicePlacement, ApiError> {
    client::request(
        reqwest::Method::POST,
        &format!("/api/v1/pois/{pin_id}/devices"),
        Some(serde_json::json!({
            "device_id": device_id,
            "location_detail": location_detail,
        })),
    )
    .await
}

/// `PATCH /api/v1/pois/placements/{id}` — édition de `location_detail`
/// et/ou **déplacement** (`{"pin_id": …}`). Le retrait passe par
/// [`delete_poi_placement`] (amendement D43, 2026-09-13).
pub async fn update_poi_placement(
    placement_id: i64,
    body: serde_json::Value,
) -> Result<DevicePlacement, ApiError> {
    client::request(
        reqwest::Method::PATCH,
        &format!("/api/v1/pois/placements/{placement_id}"),
        Some(body),
    )
    .await
}

/// `DELETE /api/v1/pois/placements/{id}` — détache le device (204) : il
/// redevient libre, plaçable n'importe où ; « au plus un POI à la fois »
/// reste garanti par l'UNIQUE côté serveur (amendement D43).
pub async fn delete_poi_placement(placement_id: i64) -> Result<Option<()>, ApiError> {
    client::request_opt(
        reqwest::Method::DELETE,
        &format!("/api/v1/pois/placements/{placement_id}"),
        None,
    )
    .await
}

/// `DELETE /api/v1/pois/{id}` — 204 (liens sources en cascade).
pub async fn delete_poi(id: &str) -> Result<Option<()>, ApiError> {
    client::request_opt(reqwest::Method::DELETE, &format!("/api/v1/pois/{id}"), None).await
}

/// `GET /api/v1/pois/cluster?bbox=west,south,east,north&zoom=…&filters`.
pub async fn cluster_pois(
    west: f64,
    south: f64,
    east: f64,
    north: f64,
    zoom: i32,
    filters: &PoiFilters,
) -> Result<ClusterResponse, ApiError> {
    let mut pairs = filters.filter_pairs();
    pairs.insert(0, format!("bbox={west:.6},{south:.6},{east:.6},{north:.6}"));
    pairs.insert(1, format!("zoom={zoom}"));
    client::request(
        reqwest::Method::GET,
        &format!("/api/v1/pois/cluster?{}", pairs.join("&")),
        None,
    )
    .await
}

// ─────────────────────────── liens (D39 → D42) ───────────────────────────
// Attach/detach passent désormais par le client générique `api::resources`
// (`POST|DELETE /api/v1/resources/edges`, thin-wrappers `/viz/links`
// legacy) — voir `pages/map.rs`.

// ─────────────────────────── positions GPS (D38) ───────────────────────────

/// `GET /api/v1/device-positions` — couche live de la carte (poll 15 s).
pub async fn device_positions() -> Result<Vec<DevicePosition>, ApiError> {
    #[derive(Deserialize)]
    struct Envelope {
        results: Vec<DevicePosition>,
    }
    let env: Envelope =
        client::request(reqwest::Method::GET, "/api/v1/device-positions", None).await?;
    Ok(env.results)
}
