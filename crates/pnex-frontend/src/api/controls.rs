//! Org controls (D125–D127): definitions, value writes from the surfaces,
//! batch read of the last commanded values. Mirror of
//! `controllers/controls.rs`.

use std::collections::BTreeMap;

use dioxus::prelude::*;
use pnex_core::ui_control::{
    ControlValue, ControlValuesRequest, ControlValuesResponse, CreateUiControl, UiControl,
    UpdateUiControl, WriteControlValue,
};
use pnex_core::Paginated;
use uuid::Uuid;

use crate::api::client;
use crate::api::error::ApiError;

/// Id → key of the org controls seen by the last [`list`] call: port labels
/// and subtitles of the `control-source` nodes (fallback: short id).
pub static CONTROL_KEYS: GlobalSignal<BTreeMap<Uuid, String>> = GlobalSignal::new(BTreeMap::new);

/// Upper bound of one listing (pickers, not an export).
const LIST_LIMIT: u32 = 500;

/// `GET /api/v1/controls` — every control of the org (refreshes
/// [`CONTROL_KEYS`]).
pub async fn list() -> Result<Vec<UiControl>, ApiError> {
    let page: Paginated<UiControl> = client::request(
        reqwest::Method::GET,
        &format!("/api/v1/controls?limit={LIST_LIMIT}"),
        None,
    )
    .await?;
    let keys: BTreeMap<Uuid, String> = page.results.iter().map(|c| (c.id, c.key.clone())).collect();
    *CONTROL_KEYS.write() = keys;
    Ok(page.results)
}

/// `POST /api/v1/controls`.
pub async fn create(params: CreateUiControl) -> Result<UiControl, ApiError> {
    client::request(
        reqwest::Method::POST,
        "/api/v1/controls",
        Some(serde_json::to_value(params).unwrap_or_default()),
    )
    .await
}

/// `PATCH /api/v1/controls/{id}`.
#[allow(dead_code)] // Wired by the surfaces (dashboards, annotations).
pub async fn update(id: Uuid, params: UpdateUiControl) -> Result<UiControl, ApiError> {
    client::request(
        reqwest::Method::PATCH,
        &format!("/api/v1/controls/{id}"),
        Some(serde_json::to_value(params).unwrap_or_default()),
    )
    .await
}

/// `DELETE /api/v1/controls/{id}` — 409 `control-in-use` while deployed
/// flows listen to it.
#[allow(dead_code)] // Wired by the controls page.
pub async fn delete(id: Uuid) -> Result<(), ApiError> {
    client::request_opt::<serde_json::Value>(
        reqwest::Method::DELETE,
        &format!("/api/v1/controls/{id}"),
        None,
    )
    .await
    .map(|_| ())
}

/// `POST /api/v1/controls/{id}/value` — operate a control from a surface
/// (`via` = `dashboard:{id}`, `annotation:{id}`). Returns the stored value.
#[allow(dead_code)] // Wired by the surfaces (dashboards, annotations).
pub async fn write_value(
    id: Uuid,
    value: f64,
    via: Option<String>,
) -> Result<ControlValue, ApiError> {
    client::request(
        reqwest::Method::POST,
        &format!("/api/v1/controls/{id}/value"),
        Some(serde_json::to_value(WriteControlValue { value, via }).unwrap_or_default()),
    )
    .await
}

/// `POST /api/v1/controls/values` — last commanded values (`None` = never
/// written).
#[allow(dead_code)] // Wired by the surfaces (dashboards, annotations).
pub async fn values(ids: Vec<Uuid>) -> Result<BTreeMap<Uuid, Option<ControlValue>>, ApiError> {
    if ids.is_empty() {
        return Ok(BTreeMap::new());
    }
    let res: ControlValuesResponse = client::request(
        reqwest::Method::POST,
        "/api/v1/controls/values",
        Some(serde_json::to_value(ControlValuesRequest { ids }).unwrap_or_default()),
    )
    .await?;
    Ok(res.values)
}

/// Display key of a control id: its key when known, else the short id.
/// Reads (and subscribes to) [`CONTROL_KEYS`]: call it from a render.
pub fn key_of(id: &Uuid) -> String {
    CONTROL_KEYS
        .read()
        .get(id)
        .cloned()
        .unwrap_or_else(|| id.to_string()[..8].to_string())
}
