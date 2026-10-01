//! Pont vers la carte géo POI-first (D27 + D35–D37) — `assets/map.js`
//! (bundle esbuild IIFE de maplibre-gl 4.x, chargé dans `main.rs`, pattern
//! viewers.js).
//!
//! Le module JS : `window.pnexMap` :
//!   - `mount(hostId, opts)` → bool : carte maplibre dans le div hôte
//!     (style Protomaps auto-hébergé, markers HTML — l'emoji couleur ne
//!     passe pas dans les glyphes SDF de maplibre) ;
//!   - `setItems(hostId, items)` : pins POI, clusters numérotés (D37) et
//!     positions GPS live (D38) ;
//!   - `flyTo(hostId, lon, lat, zoom)`, `unmount`, `error` (lib|webgl|…).
//!
//! Événements vers Rust (polling, école tron.rs : pas de closure
//! traversante, marche wasm ET natif via document::eval) :
//!   - `window.__pnexMapLastClick = {seq, kind, id, label, lng, lat}` —
//!     `kind` ∈ `pin|cluster|position|map` (clic carte nue = coords) ;
//!   - `window.__pnexMapView = {seq, west, south, east, north, zoom,
//!     center}` sur `moveend` debouncé → refetch du cluster côté page.
//!
//! Échecs silencieux (script absent, hôte introuvable, WebGL manquant) →
//! `false` : la page affiche le badge « carte indisponible » — jamais de
//! panic (école media_viewer.rs).

use serde::Deserialize;

/// Paramètres de mount de la carte globale.
pub struct MapOpts {
    pub style_url: String,
    pub center: (f64, f64),
    pub zoom: f64,
    pub items: Vec<MapItem>,
}

/// Un item affiché sur la carte (marker HTML).
#[derive(Clone, PartialEq)]
pub struct MapItem {
    /// `pin` (POI), `cluster` (numéro) ou `position` (device GPS live).
    pub kind: &'static str,
    /// Id du POI (clic → détail) — `None` pour un cluster.
    pub id: Option<String>,
    pub lon: f64,
    pub lat: f64,
    pub label: String,
    pub emoji: String,
    /// Nombre agrégé (clusters).
    pub count: u32,
}

/// Dernier clic remonté par map.js (item ou carte nue).
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

/// Viewport courant (moveend debouncé) — pilote le refetch cluster (D37).
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

/// Monte la carte dans le div `host_id` (retry 100 ms × 50 — le bundle peut
/// finir de charger après le premier rendu, école tron.rs) ; `true` si monté.
pub async fn mount(host_id: &str, opts: &MapOpts) -> bool {
    let items: Vec<serde_json::Value> = opts
        .items
        .iter()
        .map(|p| {
            serde_json::json!({
                "kind": p.kind, "id": p.id, "lon": p.lon, "lat": p.lat,
                "label": p.label, "emoji": p.emoji, "count": p.count,
            })
        })
        .collect();
    let opts_json = serde_json::json!({
        "styleUrl": opts.style_url,
        "center": [opts.center.0, opts.center.1],
        "zoom": opts.zoom,
        "items": items,
    })
    .to_string();
    mount_impl(host_id, &opts_json).await
}

/// Démonte la carte du hôte — fire-and-forget (wasm : la future est
/// consommée ; natif : l'implémentation rend `()`).
#[cfg(target_arch = "wasm32")]
pub fn unmount(host_id: &str) {
    let _ = unmount_impl(host_id);
}

#[cfg(not(target_arch = "wasm32"))]
pub fn unmount(host_id: &str) {
    unmount_impl(host_id);
}

/// Code d'erreur du dernier mount (`lib`|`webgl`|`map`|message) — pour le badge.
pub async fn error(host_id: &str) -> Option<String> {
    error_impl(host_id).await
}

/// Remplace tous les markers (items = pins/clusters/positions).
pub async fn set_items(host_id: &str, items: &[MapItem]) -> bool {
    let json: Vec<serde_json::Value> = items
        .iter()
        .map(|p| {
            serde_json::json!({
                "kind": p.kind, "id": p.id, "lon": p.lon, "lat": p.lat,
                "label": p.label, "emoji": p.emoji, "count": p.count,
            })
        })
        .collect();
    set_items_impl(host_id, &serde_json::Value::Array(json).to_string()).await
}

/// Consomme le dernier clic (poll côté page, ~300 ms) — `None` si rien de
/// neuf depuis le dernier appel.
pub async fn take_click(prev_seq: u64) -> Option<MapClick> {
    take_click_impl(prev_seq).await
}

/// Consomme le dernier viewport (poll côté page) — `None` si inchangé.
pub async fn take_viewport(prev_seq: u64) -> Option<MapViewport> {
    take_viewport_impl(prev_seq).await
}

/// Centre la carte (doux) sur un point — cluster zoomé (D37), recentrage
/// POI. Nom JS `flyTo` conservé côté pont.
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
                // apply(thisArg, args) — le NULL est le `this`, PAS un
                // argument (sinon mount reçoit (null, host, opts) et
                // getElementById(null) rend faux silencieusement).
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
    // Fire-and-forget assumé (JS synchrone côté map.js).
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

// ─────────────────────────── natif (wry) ───────────────────────────

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
    // Échappement JS des chaînes injectées (école media_viewer.rs natif).
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
