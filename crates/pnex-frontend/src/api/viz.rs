//! API client for the POI-first map (D35–D39): POI, backend clustering
//! (D37), typed links (D39), device GPS positions (D38). Mirror of
//! `controllers/pois.rs` (contract: `docs/contracts/viz.http`).

use serde::{Deserialize, Serialize};

use crate::api::client;
use crate::api::error::ApiError;
use crate::api::media::urlencode;

// ─────────────────────────── DTO ───────────────────────────

/// POI→target edge (D42: `resource_edges`, `viz_links` absorbed) —
/// the id is an integer, the `label` lives in `placement.label`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct VizLink {
    pub id: i64,
    pub source_kind: String,
    pub source_id: String,
    pub target_kind: String,
    pub target_id: String,
    pub placement: Option<serde_json::Value>,
    /// Name resolved server-side (org batch) — `None` if the target is dead or
    /// hydration failed.
    #[serde(default)]
    pub target_label: Option<String>,
    /// Target gone from the tables (deleted outside a cascade) — row
    /// disabled in the UI (D26: tolerated at render, never a panic).
    #[serde(default)]
    pub target_dead: bool,
}

/// Placement of a device on a POI (D43) — several devices per marker,
/// but a device = **a single** placement at a time (physical object).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DevicePlacement {
    pub id: i64,
    pub device_id: String,
    /// Fine location within the site ("Rack 3 — Aisle B").
    pub location_detail: Option<String>,
}

/// The POI (primary object of the map — D35).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Poi {
    pub id: String,
    pub mode: String,
    pub latitude: Option<f64>,
    pub longitude: Option<f64>,
    /// Devices placed on this POI (D43 — replaces the `device_id` column).
    #[serde(default)]
    pub devices: Vec<DevicePlacement>,
    pub label: String,
    pub emoji: String,
    pub location_detail: Option<String>,
    /// Pinned preview (000020): kind ∈ media_asset | dashboard | tour.
    #[serde(default)]
    pub preview_kind: Option<String>,
    #[serde(default)]
    pub preview_id: Option<String>,
    pub metadata: Option<serde_json::Value>,
    pub links: Vec<VizLink>,
    pub created_at: String,
    pub updated_at: String,
}

/// Live GPS position of a device (D38 — last known, source
/// `telemetry` or `manual`).
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

/// Item of the cluster response: single point (`count == 1`, `id`
/// set) or numbered cluster (D37).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ClusterItem {
    pub lat: f64,
    pub lon: f64,
    pub count: u32,
    pub id: Option<String>,
    pub label: String,
    pub emoji: String,
    #[serde(default)]
    pub color: Option<String>,
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
    /// D218: own label filter (`name` or `name:value`), list endpoint only.
    pub label: Option<String>,
    pub limit: Option<i64>,
    pub offset: Option<i64>,
}

impl PoiFilters {
    /// Filter pairs shared by list / cluster (bbox/zoom/pagination on
    /// top depending on the endpoint).
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
        if let Some(label) = self.label.as_deref().filter(|v| !v.is_empty()) {
            pairs.push(format!("label={}", urlencode(label)));
        }
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

/// `GET /api/v1/pois` — global org list (D14).
pub async fn list_pois(filters: &PoiFilters) -> Result<PoiPage, ApiError> {
    client::request(
        reqwest::Method::GET,
        &format!("/api/v1/pois{}", filters.to_query()),
        None,
    )
    .await
}

/// `POST /api/v1/pois` — creation (label + lat/lon required).
pub async fn create_poi(body: serde_json::Value) -> Result<Poi, ApiError> {
    client::request(reqwest::Method::POST, "/api/v1/pois", Some(body)).await
}

/// `GET /api/v1/pois/{id}`.
pub async fn poi_detail(id: &str) -> Result<Poi, ApiError> {
    client::request(reqwest::Method::GET, &format!("/api/v1/pois/{id}"), None).await
}

/// `PATCH /api/v1/pois/{id}` — edit/move (`null` clears: emoji,
/// location_detail; 1 PATCH on pointer-up, D34).
pub async fn update_poi(id: &str, body: serde_json::Value) -> Result<Poi, ApiError> {
    client::request(
        reqwest::Method::PATCH,
        &format!("/api/v1/pois/{id}"),
        Some(body),
    )
    .await
}

/// `POST /api/v1/pois/{id}/devices` — attaches a device (D43, deliberate).
/// 409 = device already placed: `ApiError.args` carries the current
/// placement (`current_placement_id`, `current_pin_id`, `current_pin_label`,
/// strings) so the UI offers the move knowingly.
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

/// `PATCH /api/v1/pois/placements/{id}` — edits `location_detail`
/// and/or **moves** (`{"pin_id": …}`). Removal goes through
/// [`delete_poi_placement`] (D43 amendment, 2026-09-13).
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

/// `DELETE /api/v1/pois/placements/{id}` — detaches the device (204): it
/// becomes free again, placeable anywhere; "at most one POI at a time"
/// stays guaranteed by the server-side UNIQUE (D43 amendment).
pub async fn delete_poi_placement(placement_id: i64) -> Result<Option<()>, ApiError> {
    client::request_opt(
        reqwest::Method::DELETE,
        &format!("/api/v1/pois/placements/{placement_id}"),
        None,
    )
    .await
}

/// `DELETE /api/v1/pois/{id}` — 204 (source links cascade).
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

// ─────────────────────────── links (D39 → D42) ───────────────────────────
// Attach/detach now go through the generic `api::resources` client
// (`POST|DELETE /api/v1/resources/edges`, `/viz/links` legacy
// thin-wrappers) — see `pages/map.rs`.

// ─────────────────────────── GPS positions (D38) ───────────────────────────

/// `GET /api/v1/device-positions`: live layer of the map (15 s poll),
/// every page.
pub async fn device_positions() -> Result<Vec<DevicePosition>, ApiError> {
    client::request_all("/api/v1/device-positions").await
}
