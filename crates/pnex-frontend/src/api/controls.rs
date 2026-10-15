//! Org controls (D125–D127): definitions, value writes from the surfaces,
//! batch read of the last commanded values. Mirror of
//! `controllers/controls.rs`.

use std::collections::BTreeMap;

use dioxus::prelude::*;
use pnex_core::ui_control::{
    ControlValue, ControlValuesRequest, ControlValuesResponse, CreateUiControl, UiControl,
    UpdateUiControl, WriteControlValue,
};
use uuid::Uuid;

use crate::api::client;
use crate::api::error::ApiError;

/// Id → key of the org controls seen by the last [`list`] call: port labels
/// and subtitles of the `control-source` nodes (fallback: short id).
pub static CONTROL_KEYS: GlobalSignal<BTreeMap<Uuid, String>> = GlobalSignal::new(BTreeMap::new);

/// `GET /api/v1/controls` — every control of the org (refreshes
/// [`CONTROL_KEYS`]).
pub async fn list() -> Result<Vec<UiControl>, ApiError> {
    list_filtered(None, None).await
}

/// Same as [`list`], narrowed by a search (label or key) and a label (D218).
/// A filtered call only adds to [`CONTROL_KEYS`]: it sees a subset of the org.
pub async fn list_filtered(
    search: Option<&str>,
    label: Option<&str>,
) -> Result<Vec<UiControl>, ApiError> {
    let query = client::filter_query(&[("search", search), ("label", label)]);
    let controls: Vec<UiControl> = client::request_all(&format!("/api/v1/controls{query}")).await?;
    let keys = controls.iter().map(|c| (c.id, c.key.clone()));
    if !query.is_empty() {
        CONTROL_KEYS.write().extend(keys);
    } else {
        *CONTROL_KEYS.write() = keys.collect();
    }
    Ok(controls)
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

/// Key without the surface prefix of a declared control
/// (`dash-1a2b3c4d.w-0009.power` → `w-0009.power`), for the tight port
/// labels of the canvas; standalone keys are returned as is.
pub fn short_key(key: &str) -> &str {
    match key.split_once('.') {
        Some((surface, rest))
            if !rest.is_empty()
                && ["dash-", "annot-"].iter().any(|p| {
                    surface
                        .strip_prefix(p)
                        .is_some_and(|h| h.len() == 8 && h.chars().all(|c| c.is_ascii_hexdigit()))
                }) =>
        {
            rest
        }
        _ => key,
    }
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

#[cfg(test)]
mod short_key_tests {
    use super::short_key;

    #[test]
    fn surface_prefix_is_dropped() {
        assert_eq!(short_key("dash-7d0f5f9c.w-0009.power"), "w-0009.power");
        assert_eq!(short_key("annot-1a2b3c4d.item-1"), "item-1");
        assert_eq!(short_key("garage.door"), "garage.door");
        assert_eq!(short_key("dash-zz.w-1"), "dash-zz.w-1");
    }
}
