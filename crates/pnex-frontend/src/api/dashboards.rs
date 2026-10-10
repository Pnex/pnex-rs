//! Client API des dashboards SCADA (D24/D40/D41) — miroir de
//! `controllers/dashboards.rs` + `controllers/viz_widgets.rs`.
//! Types partagés `pnex_core::viz` (la validation `validate_layout`
//! tourne côté navigateur avant save). Le 409 (save périmé) est
//! distingué via `ApiError.status`, école `api/flows.rs`.

use pnex_core::{
    CreateDashboard, CreateVizWidget, Paginated, SeriesBatchRequest, SeriesBatchResponse,
    UpdateDashboard, VizDashboard, VizDashboardSummary, VizDashboardVersion,
    VizDashboardVersionDetail, VizWidget,
};
use serde::{Deserialize, Serialize};

use crate::api::client;
use crate::api::error::ApiError;

#[derive(Debug, Default, Clone, Serialize)]
pub struct DashboardFilters {
    pub search: Option<String>,
    /// D42 effective label filter (`name` or `name:value`).
    pub label: Option<String>,
    pub limit: Option<i64>,
    pub offset: Option<i64>,
}

impl DashboardFilters {
    fn to_query(&self) -> String {
        let mut parts: Vec<String> = Vec::new();
        if let Some(v) = &self.search {
            parts.push(format!("search={}", urlencode(v)));
        }
        if let Some(v) = self.label.as_deref().filter(|v| !v.is_empty()) {
            parts.push(format!("label={}", urlencode(v)));
        }
        if let Some(v) = self.limit {
            parts.push(format!("limit={v}"));
        }
        if let Some(v) = self.offset {
            parts.push(format!("offset={v}"));
        }
        if parts.is_empty() {
            String::new()
        } else {
            format!("?{}", parts.join("&"))
        }
    }
}

fn urlencode(s: &str) -> String {
    let mut out = String::new();
    for byte in s.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(byte as char)
            }
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

/// `GET /api/v1/dashboards` — enveloppe D14.
pub async fn list(filters: &DashboardFilters) -> Result<Paginated<VizDashboardSummary>, ApiError> {
    client::request(
        reqwest::Method::GET,
        &format!("/api/v1/dashboards{}", filters.to_query()),
        None,
    )
    .await
}

/// `POST /api/v1/dashboards` — crée le dashboard + sa version 1.
pub async fn create(params: CreateDashboard) -> Result<VizDashboard, ApiError> {
    client::request(
        reqwest::Method::POST,
        "/api/v1/dashboards",
        Some(serde_json::to_value(params).unwrap_or_default()),
    )
    .await
}

/// `GET /api/v1/dashboards/{id}` — détail (layout de la version live).
pub async fn detail(id: &str) -> Result<VizDashboard, ApiError> {
    client::request(
        reqwest::Method::GET,
        &format!("/api/v1/dashboards/{id}"),
        None,
    )
    .await
}

/// `PATCH /api/v1/dashboards/{id}` = save — nouvelle version + pointeur
/// (save = live) ; 409 si `expected_version_number` est périmé, 400
/// `{"violations": […]}` si le layout est invalide.
pub async fn update(id: &str, params: UpdateDashboard) -> Result<VizDashboard, ApiError> {
    client::request(
        reqwest::Method::PATCH,
        &format!("/api/v1/dashboards/{id}"),
        Some(serde_json::to_value(params).unwrap_or_default()),
    )
    .await
}

/// `DELETE /api/v1/dashboards/{id}` — dashboard + versions (cascade).
pub async fn delete(id: &str) -> Result<(), ApiError> {
    client::request_opt::<serde_json::Value>(
        reqwest::Method::DELETE,
        &format!("/api/v1/dashboards/{id}"),
        None,
    )
    .await
    .map(|_| ())
}

/// `GET /api/v1/dashboards/{id}/versions` — historique append-only.
pub async fn versions(id: &str) -> Result<Vec<VizDashboardVersion>, ApiError> {
    client::request(
        reqwest::Method::GET,
        &format!("/api/v1/dashboards/{id}/versions"),
        None,
    )
    .await
}

/// `GET /api/v1/dashboards/{id}/versions/{n}` — layout d'une version.
pub async fn version_detail(
    id: &str,
    version_number: i64,
) -> Result<VizDashboardVersionDetail, ApiError> {
    client::request(
        reqwest::Method::GET,
        &format!("/api/v1/dashboards/{id}/versions/{version_number}"),
        None,
    )
    .await
}

/// `POST /api/v1/dashboards/{id}/versions/{n}/restore` — re-positionne
/// la version live (aucune version créée).
pub async fn restore(id: &str, version_number: i64) -> Result<VizDashboard, ApiError> {
    client::request::<VizDashboard>(
        reqwest::Method::POST,
        &format!("/api/v1/dashboards/{id}/versions/{version_number}/restore"),
        None,
    )
    .await
}

// ─────────────────────────── Bibliothèque (D41) ───────────────────────────

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct WidgetLibraryFilters {
    pub search: Option<String>,
    pub kind: Option<String>,
}

impl WidgetLibraryFilters {
    fn to_query(&self) -> String {
        let mut parts: Vec<String> = Vec::new();
        if let Some(v) = &self.search {
            parts.push(format!("search={}", urlencode(v)));
        }
        if let Some(v) = &self.kind {
            parts.push(format!("kind={}", urlencode(v)));
        }
        if parts.is_empty() {
            String::new()
        } else {
            format!("?{}", parts.join("&"))
        }
    }
}

/// `GET /api/v1/viz/widgets` — templates de l'org.
pub async fn widgets(filters: &WidgetLibraryFilters) -> Result<Paginated<VizWidget>, ApiError> {
    client::request(
        reqwest::Method::GET,
        &format!("/api/v1/viz/widgets{}", filters.to_query()),
        None,
    )
    .await
}

/// `POST /api/v1/viz/widgets` — ajoute un template à la bibliothèque.
pub async fn create_widget(params: CreateVizWidget) -> Result<VizWidget, ApiError> {
    client::request(
        reqwest::Method::POST,
        "/api/v1/viz/widgets",
        Some(serde_json::to_value(params).unwrap_or_default()),
    )
    .await
}

/// `DELETE /api/v1/viz/widgets/{id}` — les dashboards gardent leur
/// snapshot (aucune cascade métier).
pub async fn delete_widget(id: &str) -> Result<(), ApiError> {
    client::request_opt::<serde_json::Value>(
        reqwest::Method::DELETE,
        &format!("/api/v1/viz/widgets/{id}"),
        None,
    )
    .await
    .map(|_| ())
}

// ─────────────────────────── Télémétrie batch (D31) ───────────────────────────

/// `POST /api/v1/telemetry/series-batch` — toutes les sources d'un
/// dashboard en un appel (un timer 15 s au lieu de N, D31) ; dégradation
/// **par item** (`available: false`), jamais d'échec global.
pub async fn series_batch(body: SeriesBatchRequest) -> Result<SeriesBatchResponse, ApiError> {
    client::request(
        reqwest::Method::POST,
        "/api/v1/telemetry/series-batch",
        Some(serde_json::to_value(body).unwrap_or_default()),
    )
    .await
}

/// `POST /api/v1/telemetry/aggregate` (D182) — one value per time range or
/// time-of-day slice; `available: false` when O2 is out of reach.
pub async fn aggregate(
    body: &pnex_core::aggregate::AggregateRequest,
) -> Result<pnex_core::aggregate::AggregateResponse, ApiError> {
    client::request(
        reqwest::Method::POST,
        "/api/v1/telemetry/aggregate",
        Some(serde_json::to_value(body).unwrap_or_default()),
    )
    .await
}
