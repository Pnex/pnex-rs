//! API client for SCADA dashboards (D24/D40/D41) — mirror of
//! `controllers/dashboards.rs` + `controllers/viz_widgets.rs`.
//! Shared types `pnex_core::viz` (`validate_layout` validation
//! runs in the browser before save). The 409 (stale save) is
//! told apart via `ApiError.status`, `api/flows.rs` style.

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

/// `GET /api/v1/dashboards` — D14 envelope.
pub async fn list(filters: &DashboardFilters) -> Result<Paginated<VizDashboardSummary>, ApiError> {
    client::request(
        reqwest::Method::GET,
        &format!("/api/v1/dashboards{}", filters.to_query()),
        None,
    )
    .await
}

/// `POST /api/v1/dashboards` — creates the dashboard + its version 1.
pub async fn create(params: CreateDashboard) -> Result<VizDashboard, ApiError> {
    client::request(
        reqwest::Method::POST,
        "/api/v1/dashboards",
        Some(serde_json::to_value(params).unwrap_or_default()),
    )
    .await
}

/// `GET /api/v1/dashboards/{id}` — detail (layout of the live version).
pub async fn detail(id: &str) -> Result<VizDashboard, ApiError> {
    client::request(
        reqwest::Method::GET,
        &format!("/api/v1/dashboards/{id}"),
        None,
    )
    .await
}

/// `PATCH /api/v1/dashboards/{id}` = save — new version + pointer
/// (save = live); 409 if `expected_version_number` is stale, 400
/// `{"violations": […]}` if the layout is invalid.
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

/// `GET /api/v1/dashboards/{id}/versions` (all pages): append-only history.
pub async fn versions(id: &str) -> Result<Vec<VizDashboardVersion>, ApiError> {
    client::request_all(&format!("/api/v1/dashboards/{id}/versions")).await
}

/// `GET /api/v1/dashboards/{id}/versions/{n}` — layout of a version.
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

/// `POST /api/v1/dashboards/{id}/versions/{n}/restore` — re-points
/// the live version (no version created).
pub async fn restore(id: &str, version_number: i64) -> Result<VizDashboard, ApiError> {
    client::request::<VizDashboard>(
        reqwest::Method::POST,
        &format!("/api/v1/dashboards/{id}/versions/{version_number}/restore"),
        None,
    )
    .await
}

// ─────────────────────────── Library (D41) ───────────────────────────

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

/// `GET /api/v1/viz/widgets` (all pages): templates of the org library.
pub async fn widgets(filters: &WidgetLibraryFilters) -> Result<Vec<VizWidget>, ApiError> {
    client::request_all(&format!("/api/v1/viz/widgets{}", filters.to_query())).await
}

/// `POST /api/v1/viz/widgets` — adds a template to the library.
pub async fn create_widget(params: CreateVizWidget) -> Result<VizWidget, ApiError> {
    client::request(
        reqwest::Method::POST,
        "/api/v1/viz/widgets",
        Some(serde_json::to_value(params).unwrap_or_default()),
    )
    .await
}

/// `DELETE /api/v1/viz/widgets/{id}` — dashboards keep their
/// snapshot (no business cascade).
pub async fn delete_widget(id: &str) -> Result<(), ApiError> {
    client::request_opt::<serde_json::Value>(
        reqwest::Method::DELETE,
        &format!("/api/v1/viz/widgets/{id}"),
        None,
    )
    .await
    .map(|_| ())
}

// ─────────────────────────── Batch telemetry (D31) ───────────────────────────

/// `POST /api/v1/telemetry/series/query` — every source of a
/// dashboard in one call (one 15 s timer instead of N, D31); degrades
/// **per item** (`available: false`), never a global failure.
pub async fn series_batch(body: SeriesBatchRequest) -> Result<SeriesBatchResponse, ApiError> {
    client::request(
        reqwest::Method::POST,
        "/api/v1/telemetry/series/query",
        Some(serde_json::to_value(body).unwrap_or_default()),
    )
    .await
}

/// `POST /api/v1/telemetry/aggregate/query` (D182) — one value per time range or
/// time-of-day slice; `available: false` when O2 is out of reach.
pub async fn aggregate(
    body: &pnex_core::aggregate::AggregateRequest,
) -> Result<pnex_core::aggregate::AggregateResponse, ApiError> {
    client::request(
        reqwest::Method::POST,
        "/api/v1/telemetry/aggregate/query",
        Some(serde_json::to_value(body).unwrap_or_default()),
    )
    .await
}
