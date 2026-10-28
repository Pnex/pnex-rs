//! Bridge to the POI-first geo map (D27 + D35–D37) — `assets/map.js`
//! (esbuild IIFE bundle of maplibre-gl 4.x, loaded in `main.rs`, viewers.js
//! pattern).
//!
//! The JS module: `window.pnexMap`:
//!   - `mount(hostId, opts)` → bool: maplibre map in the host div
//!     (self-hosted Protomaps style, HTML markers — colour emoji does
//!     not fit maplibre's SDF glyphs);
//!   - `setItems(hostId, items)`: POI pins, numbered clusters (D37) and
//!     live GPS positions (D38);
//!   - `flyTo(hostId, lon, lat, zoom)`, `unmount`, `error` (lib|webgl|…).
//!
//! Events to Rust (polling, tron.rs school: no crossing closure,
//! works on wasm AND native via document::eval):
//!   - `window.__pnexMapLastClick = {seq, kind, id, label, lng, lat}` —
//!     `kind` ∈ `pin|cluster|position|map` (bare map click = coords);
//!   - `window.__pnexMapView = {seq, west, south, east, north, zoom,
//!     center}` on debounced `moveend` → cluster refetch on the page side.
//!
//! Silent failures (missing script, host not found, missing WebGL) →
//! `false`: the page shows the "map unavailable" badge — never a
//! panic (media_viewer.rs school).

use serde::Deserialize;

/// Mount parameters of the global map.
pub struct MapOpts {
    pub style_url: String,
    /// Style used while the app theme is dark (the basemap's dark variant).
    pub style_url_dark: Option<String>,
    pub center: (f64, f64),
    pub zoom: f64,
    pub items: Vec<MapItem>,
}

/// An item shown on the map (HTML marker).
#[derive(Clone, PartialEq)]
pub struct MapItem {
    /// `pin` (POI), `cluster` (number) or `position` (live GPS device).
    pub kind: &'static str,
    /// POI id (click → detail) — `None` for a cluster.
    pub id: Option<String>,
    pub lon: f64,
    pub lat: f64,
    pub label: String,
    pub emoji: String,
    /// Aggregated count (clusters).
    pub count: u32,
    /// Colour token of the pin's status (D339): `green`, `amber`…
    pub color: Option<String>,
}

/// Last click reported by map.js (item or bare map).
#[derive(Debug, Deserialize)]
pub struct MapClick {
    pub seq: u64,
    /// `pin`|`cluster`|`position`|`map`.
    pub kind: String,
    pub id: Option<String>,
    #[allow(dead_code)]
    pub label: String,
    /// Coords (clic carte nue).
    pub lng: Option<f64>,
    pub lat: Option<f64>,
}

/// Current viewport (debounced moveend) — drives the cluster refetch (D37).
#[derive(Debug, Deserialize)]
pub struct MapViewport {
    pub seq: u64,
    #[allow(dead_code)]
    pub host: String,
    pub west: f64,
    pub south: f64,
    pub east: f64,
    pub north: f64,
    pub zoom: f64,
    #[allow(dead_code)]
    pub center: (f64, f64),
}

/// Mounts the map in the `host_id` div (retry 100 ms × 50 — the bundle may
/// finish loading after the first render, tron.rs school); `true` if mounted.
pub async fn mount(host_id: &str, opts: &MapOpts) -> bool {
    let items: Vec<serde_json::Value> = opts
        .items
        .iter()
        .map(|p| {
            serde_json::json!({
                "kind": p.kind, "id": p.id, "lon": p.lon, "lat": p.lat,
                "label": p.label, "emoji": p.emoji, "count": p.count, "color": p.color,
            })
        })
        .collect();
    let opts_json = serde_json::json!({
        "styleUrl": opts.style_url,
        "styleUrlDark": opts.style_url_dark,
        "center": [opts.center.0, opts.center.1],
        "zoom": opts.zoom,
        "items": items,
    })
    .to_string();
    mount_impl(host_id, &opts_json).await
}

/// Unmounts the map from the host — fire-and-forget (wasm: the future is
/// consumed; native: the implementation returns `()`).
#[cfg(target_arch = "wasm32")]
pub fn unmount(host_id: &str) {
    let _ = unmount_impl(host_id);
}

#[cfg(not(target_arch = "wasm32"))]
pub fn unmount(host_id: &str) {
    unmount_impl(host_id);
}

/// Error code of the last mount (`lib`|`webgl`|`map`|message) — for the badge.
pub async fn error(host_id: &str) -> Option<String> {
    error_impl(host_id).await
}

/// Replaces all markers (items = pins/clusters/positions).
pub async fn set_items(host_id: &str, items: &[MapItem]) -> bool {
    let json: Vec<serde_json::Value> = items
        .iter()
        .map(|p| {
            serde_json::json!({
                "kind": p.kind, "id": p.id, "lon": p.lon, "lat": p.lat,
                "label": p.label, "emoji": p.emoji, "count": p.count, "color": p.color,
            })
        })
        .collect();
    set_items_impl(host_id, &serde_json::Value::Array(json).to_string()).await
}

/// Tracks of a window (cases-workspace.md K28), sent once: `data` is
/// `{recentMs, tracks: [{id, color, segments: [[[lon, lat, t_ms]]]}]}`.
pub async fn set_tracks(host_id: &str, data: &serde_json::Value) -> bool {
    call_json(host_id, "setTracks", &data.to_string()).await
}

/// Areas to draw, a GeoJSON FeatureCollection of polygons (K29).
pub async fn set_zones(host_id: &str, data: &serde_json::Value) -> bool {
    call_json(host_id, "setZones", &data.to_string()).await
}

/// Cursor instant (ms): map.js redraws the past of each track only.
pub async fn set_time(host_id: &str, t_ms: i64) -> bool {
    set_time_impl(host_id, t_ms).await
}

/// Consumes the last click (polled by the page, ~300 ms) — `None` if nothing
/// new since the last call.
pub async fn take_click(prev_seq: u64) -> Option<MapClick> {
    take_click_impl(prev_seq).await
}

/// Consumes the last viewport (polled by the page) — `None` if unchanged.
pub async fn take_viewport(prev_seq: u64) -> Option<MapViewport> {
    take_viewport_impl(prev_seq).await
}

/// Centres the map (smoothly) on a point — zoomed cluster (D37), POI
/// recentring. JS name `flyTo` kept on the bridge side.
#[allow(non_snake_case)]
pub async fn flyTo(host_id: &str, lon: f64, lat: f64, zoom: f64) -> bool {
    fly_to_impl(host_id, lon, lat, zoom).await
}

// ─────────────────────────── wasm32 ───────────────────────────

#[cfg(target_arch = "wasm32")]
mod wasm {
    use wasm_bindgen::{JsCast, JsValue};

    pub fn global_method(object: &str, method: &str) -> Option<JsValue> {
        js_sys::Reflect::get(&js_sys::global(), &JsValue::from_str(object))
            .ok()
            .and_then(|value| js_sys::Reflect::get(&value, &JsValue::from_str(method)).ok())
    }

    pub fn call(object: &str, method: &str, args: &[JsValue]) -> Option<JsValue> {
        global_method(object, method)
            .and_then(|f| f.dyn_into::<js_sys::Function>().ok())
            .and_then(|f| {
                // apply(thisArg, args) — the NULL is the `this`, NOT an
                // argument (otherwise mount receives (null, host, opts) and
                // getElementById(null) silently returns false).
                f.apply(&JsValue::NULL, &js_sys::Array::from_iter(args.iter()))
                    .ok()
            })
    }

    pub fn global_value(name: &str) -> Option<JsValue> {
        js_sys::Reflect::get(&js_sys::global(), &JsValue::from_str(name)).ok()
    }
}

#[cfg(target_arch = "wasm32")]
async fn mount_impl(host_id: &str, opts_json: &str) -> bool {
    for _ in 0..50 {
        if try_mount(host_id, opts_json) {
            return true;
        }
        gloo_timers::future::TimeoutFuture::new(100).await;
    }
    false
}

#[cfg(target_arch = "wasm32")]
fn try_mount(host_id: &str, opts_json: &str) -> bool {
    use wasm_bindgen::JsValue;
    wasm::call(
        "pnexMap",
        "mount",
        &[JsValue::from_str(host_id), JsValue::from_str(opts_json)],
    )
    .and_then(|v| v.as_bool())
    .unwrap_or(false)
}

#[cfg(target_arch = "wasm32")]
fn unmount_impl(host_id: &str) {
    // Deliberate fire-and-forget (synchronous JS in map.js).
    wasm::call(
        "pnexMap",
        "unmount",
        &[wasm_bindgen::JsValue::from_str(host_id)],
    );
}

#[cfg(target_arch = "wasm32")]
async fn error_impl(host_id: &str) -> Option<String> {
    wasm::call(
        "pnexMap",
        "error",
        &[wasm_bindgen::JsValue::from_str(host_id)],
    )
    .and_then(|v| v.as_string())
    .filter(|e| !e.is_empty())
}

#[cfg(target_arch = "wasm32")]
async fn set_items_impl(host_id: &str, items_json: &str) -> bool {
    use wasm_bindgen::JsValue;
    wasm::call(
        "pnexMap",
        "setItems",
        &[JsValue::from_str(host_id), JsValue::from_str(items_json)],
    )
    .and_then(|v| v.as_bool())
    .unwrap_or(false)
}

#[cfg(target_arch = "wasm32")]
async fn call_json(host_id: &str, method: &str, json: &str) -> bool {
    use wasm_bindgen::JsValue;
    wasm::call(
        "pnexMap",
        method,
        &[JsValue::from_str(host_id), JsValue::from_str(json)],
    )
    .and_then(|v| v.as_bool())
    .unwrap_or(false)
}

#[cfg(target_arch = "wasm32")]
async fn set_time_impl(host_id: &str, t_ms: i64) -> bool {
    use wasm_bindgen::JsValue;
    wasm::call(
        "pnexMap",
        "setTime",
        &[JsValue::from_str(host_id), JsValue::from_f64(t_ms as f64)],
    )
    .and_then(|v| v.as_bool())
    .unwrap_or(false)
}

#[cfg(target_arch = "wasm32")]
async fn take_click_impl(prev_seq: u64) -> Option<MapClick> {
    let raw = wasm::global_value("__pnexMapLastClick")?.as_string()?;
    let click: MapClick = serde_json::from_str(&raw).ok()?;
    (click.seq > prev_seq).then_some(click)
}

#[cfg(target_arch = "wasm32")]
async fn take_viewport_impl(prev_seq: u64) -> Option<MapViewport> {
    let raw = wasm::global_value("__pnexMapView")?.as_string()?;
    let view: MapViewport = serde_json::from_str(&raw).ok()?;
    (view.seq > prev_seq).then_some(view)
}

#[cfg(target_arch = "wasm32")]
async fn fly_to_impl(host_id: &str, lon: f64, lat: f64, zoom: f64) -> bool {
    use wasm_bindgen::JsValue;
    wasm::call(
        "pnexMap",
        "flyTo",
        &[
            JsValue::from_str(host_id),
            JsValue::from_f64(lon),
            JsValue::from_f64(lat),
            JsValue::from_f64(zoom),
        ],
    )
    .and_then(|v| v.as_bool())
    .unwrap_or(false)
}

// ─────────────────────────── native (wry) ───────────────────────────

#[cfg(not(target_arch = "wasm32"))]
async fn mount_impl(host_id: &str, opts_json: &str) -> bool {
    for _ in 0..50 {
        if try_mount(host_id, opts_json).await {
            return true;
        }
        crate::util::sleep(std::time::Duration::from_millis(100)).await;
    }
    false
}

#[cfg(not(target_arch = "wasm32"))]
async fn try_mount(host_id: &str, opts_json: &str) -> bool {
    // JS escaping of injected strings (native media_viewer.rs school).
    let host_id_q = host_id.replace('\'', "");
    let opts_q = opts_json.replace('\\', "\\\\").replace('\'', "\\'");
    let js =
        format!("return window.pnexMap?.mount('{host_id_q}', JSON.parse('{opts_q}')) === true");
    eval_bool(&js).await
}

#[cfg(not(target_arch = "wasm32"))]
async fn eval_bool(js: &str) -> bool {
    dioxus::document::eval(js)
        .await
        .ok()
        .and_then(|value| {
            value
                .as_bool()
                .or(value.as_str().map(str::parse::<bool>).and_then(|r| r.ok()))
        })
        .unwrap_or(false)
}

#[cfg(not(target_arch = "wasm32"))]
fn unmount_impl(host_id: &str) {
    let host_id_q = host_id.replace('\'', "");
    dioxus::prelude::spawn(async move {
        let _ = dioxus::document::eval(&format!("window.pnexMap?.unmount('{host_id_q}')")).await;
    });
}

#[cfg(not(target_arch = "wasm32"))]
async fn error_impl(host_id: &str) -> Option<String> {
    let host_id_q = host_id.replace('\'', "");
    let js = format!("return window.pnexMap?.error('{host_id_q}') || null");
    dioxus::document::eval(&js)
        .await
        .ok()
        .and_then(|value| value.as_str().map(str::to_string))
        .filter(|e| !e.is_empty() && e != "null")
}

#[cfg(not(target_arch = "wasm32"))]
async fn set_items_impl(host_id: &str, items_json: &str) -> bool {
    let host_id_q = host_id.replace('\'', "");
    let items_q = items_json.replace('\\', "\\\\").replace('\'', "\\'");
    let js =
        format!("return window.pnexMap?.setItems('{host_id_q}', JSON.parse('{items_q}')) === true");
    eval_bool(&js).await
}

#[cfg(not(target_arch = "wasm32"))]
async fn call_json(host_id: &str, method: &str, json: &str) -> bool {
    let host_id_q = host_id.replace('\'', "");
    let json_q = json.replace('\\', "\\\\").replace('\'', "\\'");
    let js =
        format!("return window.pnexMap?.{method}('{host_id_q}', JSON.parse('{json_q}')) === true");
    eval_bool(&js).await
}

#[cfg(not(target_arch = "wasm32"))]
async fn set_time_impl(host_id: &str, t_ms: i64) -> bool {
    let host_id_q = host_id.replace('\'', "");
    eval_bool(&format!(
        "return window.pnexMap?.setTime('{host_id_q}', {t_ms}) === true"
    ))
    .await
}

#[cfg(not(target_arch = "wasm32"))]
async fn take_click_impl(prev_seq: u64) -> Option<MapClick> {
    let js = "return window.__pnexMapLastClick || null";
    let raw = dioxus::document::eval(js)
        .await
        .ok()
        .and_then(|value| value.as_str().map(str::to_string))?;
    let click: MapClick = serde_json::from_str(&raw).ok()?;
    (click.seq > prev_seq).then_some(click)
}

#[cfg(not(target_arch = "wasm32"))]
async fn take_viewport_impl(prev_seq: u64) -> Option<MapViewport> {
    let js = "return window.__pnexMapView || null";
    let raw = dioxus::document::eval(js)
        .await
        .ok()
        .and_then(|value| value.as_str().map(str::to_string))?;
    let view: MapViewport = serde_json::from_str(&raw).ok()?;
    (view.seq > prev_seq).then_some(view)
}

#[cfg(not(target_arch = "wasm32"))]
async fn fly_to_impl(host_id: &str, lon: f64, lat: f64, zoom: f64) -> bool {
    let host_id_q = host_id.replace('\'', "");
    let js = format!("return window.pnexMap?.flyTo('{host_id_q}', {lon}, {lat}, {zoom}) === true");
    eval_bool(&js).await
}
