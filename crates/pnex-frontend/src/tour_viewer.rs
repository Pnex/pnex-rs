//! Pont vers le viewer de tour — `assets/viewers.js` expose
//! `window.pnexViewers.tour = {mount, switch, nav, unmount}` (pannellum
//! unique, décision S12). École `media_viewer.rs` : retry 100 ms × 50,
//! échec → `false` (badge « aperçu indisponible », jamais de panique) ;
//! wasm = `js_sys::Reflect`, natif = `document::eval` (glue **synchrone** :
//! le mount pannellum ne renvoie pas de promesse).
//!
//! Navigation par hotspots : la closure JS pose un global **chaîne JSON**
//! `window.__pnexTourLastNav` (`{seq, scene_id, floor_id}`) — Rust **poll**
//! via [`take_nav`] (pattern global-chaîne-JSON de tron/map) ; le seq
//! monotone évite de rejouer une navigation déjà consommée.

use serde::{Deserialize, Serialize};

/// Hotspot d'une scène posée dans le viewer (position sphère + cible).
#[derive(Debug, Clone, Serialize)]
pub struct TourHotspotView {
    pub yaw: f64,
    pub pitch: f64,
    pub target_scene: String,
    pub target_floor: String,
    pub label: String,
    /// `walk` | `stair` (dérivé) — choisit la flèche PNeX côté glue JS.
    pub kind: String,
    /// Id du lien du doc — cible du drag de repositionnement.
    pub link_id: String,
}

/// Scène montable : URL blob/data-URI **déjà résolue** (le fetch d'octets
/// reste côté Rust — la glue JS est synchrone), vue initiale + hotspots.
#[derive(Debug, Clone, Serialize)]
pub struct TourSceneView {
    pub scene_id: String,
    pub floor_id: String,
    pub url: String,
    pub yaw: f64,
    pub pitch: f64,
    pub hfov: f64,
    pub hotspots: Vec<TourHotspotView>,
}

/// Navigation remontée par un clic hotspot.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct TourNav {
    pub seq: u64,
    pub scene_id: String,
    pub floor_id: String,
}

/// Repositionnement d'un hotspot (drag en preview) remonté au doc.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct HotspotMove {
    pub seq: u64,
    pub link_id: String,
    pub yaw: f64,
    pub pitch: f64,
}

// ───────────────────────── Annotations (D55–D60) ─────────────────────────

/// Marqueur d'annotation d'une scène posé dans le viewer (D58) — hotspot
/// pannellum `pnex-annot pnex-annot-{kind}`, classes **strictement
/// disjointes** de `.pnex-hotspot` (le drag nav résout par miroir index).
#[derive(Debug, Clone, Serialize)]
pub struct AnnotMarkerView {
    /// Id d'item du doc (la glue préfixe `annot-` pour le DOM).
    pub id: String,
    pub yaw: f64,
    pub pitch: f64,
    /// `device|pin|status|note` — choisit la couleur du marqueur.
    pub kind: String,
    pub label: String,
}

/// Clic d'un marqueur d'annotation (global `__pnexAnnotClick`).
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct AnnotClick {
    pub seq: u64,
    pub item_id: String,
}

/// Pose d'un marqueur (clic sur le pano hors marqueur, mode éditeur —
/// global `__pnexAnnotPlace`). Panorama: `yaw`/`pitch`
/// (`mouseEventToCoords`); splat: the picked world point `x`/`y`/`z`.
/// Plain optional fields (no flatten/untagged: arbitrary_precision).
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct AnnotPlace {
    pub seq: u64,
    #[serde(default)]
    pub yaw: Option<f64>,
    #[serde(default)]
    pub pitch: Option<f64>,
    #[serde(default)]
    pub x: Option<f64>,
    #[serde(default)]
    pub y: Option<f64>,
    #[serde(default)]
    pub z: Option<f64>,
}

/// Drag d'ajustement d'un marqueur (mode éditeur — global
/// `__pnexAnnotMove`), same coordinates as [`AnnotPlace`].
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct AnnotMove {
    pub seq: u64,
    pub item_id: String,
    #[serde(default)]
    pub yaw: Option<f64>,
    #[serde(default)]
    pub pitch: Option<f64>,
    #[serde(default)]
    pub x: Option<f64>,
    #[serde(default)]
    pub y: Option<f64>,
    #[serde(default)]
    pub z: Option<f64>,
}

/// Geometry carried by a place/move event (`None` if incomplete).
fn event_geometry(
    yaw: Option<f64>,
    pitch: Option<f64>,
    x: Option<f64>,
    y: Option<f64>,
    z: Option<f64>,
) -> Option<pnex_core::AnnotationGeometry> {
    match (yaw, pitch, x, y, z) {
        (Some(yaw), Some(pitch), ..) => {
            Some(pnex_core::AnnotationGeometry::Equirect { yaw, pitch })
        }
        (_, _, Some(x), Some(y), Some(z)) => Some(pnex_core::AnnotationGeometry::Splat { x, y, z }),
        _ => None,
    }
}

impl AnnotPlace {
    pub fn geometry(&self) -> Option<pnex_core::AnnotationGeometry> {
        event_geometry(self.yaw, self.pitch, self.x, self.y, self.z)
    }
}

impl AnnotMove {
    pub fn geometry(&self) -> Option<pnex_core::AnnotationGeometry> {
        event_geometry(self.yaw, self.pitch, self.x, self.y, self.z)
    }
}

/// Splat marker (world point) set on a mounted splat viewer.
#[derive(Debug, Clone, Serialize)]
pub struct SplatMarkerView {
    pub id: String,
    pub x: f64,
    pub y: f64,
    pub z: f64,
    pub kind: String,
    pub label: String,
}

/// Viewer whose annotation glue is called (`pnexViewers.<ns>`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum AnnotNs {
    /// Pannellum (panoramas, tours).
    Tour,
    /// gsplat (gaussian splats).
    Splat,
}

impl AnnotNs {
    fn as_str(self) -> &'static str {
        match self {
            AnnotNs::Tour => "tour",
            AnnotNs::Splat => "splat",
        }
    }
}

/// Argument of a glue call.
enum JsArg {
    Str(String),
    Bool(bool),
}

/// Calls `pnexViewers.<ns>.<method>(args…)` with the mount retry contract
/// (100 ms × 50) — `true` once the glue answered `true`.
async fn call_with_retry(ns: &str, method: &str, args: &[JsArg]) -> bool {
    for _ in 0..50 {
        if try_call_once(ns, method, args).await {
            return true;
        }
        sleep_ms(100).await;
    }
    false
}

/// Splat markers (post-mount, diffed by id in the glue). `editable` enables
/// the marker drag (editor only).
pub async fn set_splat_annotations(
    host_id: &str,
    markers: &[SplatMarkerView],
    editable: bool,
) -> bool {
    let json = serde_json::to_string(markers).unwrap_or_default();
    call_with_retry(
        AnnotNs::Splat.as_str(),
        "setAnnotations",
        &[
            JsArg::Str(host_id.to_string()),
            JsArg::Str(json),
            JsArg::Bool(editable),
        ],
    )
    .await
}

/// Place mode of a viewer (click → `__pnexAnnotPlace`).
pub async fn set_place_mode(ns: AnnotNs, host_id: &str, on: bool) -> bool {
    call_with_retry(
        ns.as_str(),
        "setAnnotPlaceMode",
        &[JsArg::Str(host_id.to_string()), JsArg::Bool(on)],
    )
    .await
}

/// Cards that follow their markers (D147): the glue positions every
/// `[data-annot-card]` of the overlay `overlay_id` next to its marker in
/// the viewer `host_id` (panorama hotspot or projected splat point).
pub async fn follow_cards(host_id: &str, overlay_id: &str, on: bool) -> bool {
    call_with_retry(
        "cards",
        "follow",
        &[
            JsArg::Str(host_id.to_string()),
            JsArg::Str(overlay_id.to_string()),
            JsArg::Bool(on),
        ],
    )
    .await
}

/// Monte la scène dans le div `host_id`. Retry 100 ms × 50 (le bundle peut
/// finir de charger après le premier rendu) ; `true` si monté.
pub async fn mount(host_id: &str, scene: &TourSceneView) -> bool {
    for _ in 0..50 {
        if try_scene_once("mount", host_id, scene).await {
            return true;
        }
        sleep_ms(100).await;
    }
    false
}

/// Change de scène : destroy + re-mount (S13 — une seule texture en mémoire).
/// Même retry que le mount : un échec transitoire (contexte WebGL pas encore
/// libéré par le GC, bundle en cours) sinon laisse un viewer vide sans
/// retour — le composant affiche le badge `load_failed` si tout échoue.
pub async fn switch_scene(host_id: &str, scene: &TourSceneView) -> bool {
    for _ in 0..50 {
        if try_scene_once("switchScene", host_id, scene).await {
            return true;
        }
        sleep_ms(100).await;
    }
    false
}

/// Démonte le viewer du hôte — fire-and-forget.
pub fn unmount(host_id: &str) {
    unmount_impl(host_id);
}

/// Lit la dernière navigation hotspot : `Some` si `seq` > `prev_seq` —
/// l'appelant mémorise le seq consommé.
pub async fn take_nav(prev_seq: u64) -> Option<TourNav> {
    let raw = read_nav_global().await?;
    let nav: TourNav = serde_json::from_str(&raw).ok()?;
    (nav.seq > prev_seq).then_some(nav)
}

/// Lit le dernier drag de hotspot : même contrat seq que [`take_nav`].
pub async fn take_hotspot_move(prev_seq: u64) -> Option<HotspotMove> {
    let raw = read_move_global().await?;
    let mv: HotspotMove = serde_json::from_str(&raw).ok()?;
    (mv.seq > prev_seq).then_some(mv)
}

/// Pose/remplace les marqueurs d'annotations de la scène montée
/// (**post-mount** — `setAnnotations`, remove/add de hotspots, JAMAIS de
/// re-mount du viewer : flicker + rechargement texture, D58). Retry
/// 100 ms × 50 comme le mount (viewer pas encore prêt au premier tick).
/// `editable` active le drag d'ajustement (mode éditeur uniquement).
pub async fn set_annotations(host_id: &str, markers: &[AnnotMarkerView], editable: bool) -> bool {
    let json = serde_json::to_string(markers).unwrap_or_default();
    call_with_retry(
        AnnotNs::Tour.as_str(),
        "setAnnotations",
        &[
            JsArg::Str(host_id.to_string()),
            JsArg::Str(json),
            JsArg::Bool(editable),
        ],
    )
    .await
}

/// Gated marker (arrow) drag: true only in the tour editor preview —
/// read-only viewers (map POI preview, share page, annotation page) keep
/// markers fixed. Same retry contract as [`set_annot_place_mode`].
pub async fn set_tour_editable(host_id: &str, on: bool) -> bool {
    for _ in 0..50 {
        if try_set_tour_editable_once(host_id, on).await {
            return true;
        }
        sleep_ms(100).await;
    }
    false
}

/// Lit le dernier clic de marqueur : contrat seq de [`take_nav`].
pub async fn take_annot_click(prev_seq: u64) -> Option<AnnotClick> {
    let raw = read_annot_global("__pnexAnnotClick").await?;
    let click: AnnotClick = serde_json::from_str(&raw).ok()?;
    (click.seq > prev_seq).then_some(click)
}

/// Lit la dernière pose de marqueur : contrat seq de [`take_nav`].
pub async fn take_annot_place(prev_seq: u64) -> Option<AnnotPlace> {
    let raw = read_annot_global("__pnexAnnotPlace").await?;
    let place: AnnotPlace = serde_json::from_str(&raw).ok()?;
    (place.seq > prev_seq).then_some(place)
}

/// Lit le dernier drag de marqueur : contrat seq de [`take_nav`].
pub async fn take_annot_move(prev_seq: u64) -> Option<AnnotMove> {
    let raw = read_annot_global("__pnexAnnotMove").await?;
    let mv: AnnotMove = serde_json::from_str(&raw).ok()?;
    (mv.seq > prev_seq).then_some(mv)
}

// ─────────────────────────── wasm32 (web) ───────────────────────────

#[cfg(target_arch = "wasm32")]
async fn try_scene_once(method: &str, host_id: &str, scene: &TourSceneView) -> bool {
    let _ = log_device(method);
    let json = serde_json::to_string(scene).unwrap_or_default();
    let Some(f) = global_method("pnexViewers", "tour")
        .and_then(|tour| js_sys::Reflect::get(&tour, &wasm_bindgen::JsValue::from_str(method)).ok())
        .and_then(|v| v.dyn_into::<js_sys::Function>().ok())
    else {
        return false;
    };
    f.call2(
        &wasm_bindgen::JsValue::NULL,
        &wasm_bindgen::JsValue::from_str(host_id),
        &wasm_bindgen::JsValue::from_str(&json),
    )
    .ok()
    .and_then(|value| value.as_bool())
    .unwrap_or(false)
}

/// Résout `window.<objet>` (globales posées par viewers.js).
#[cfg(target_arch = "wasm32")]
fn global_method(object: &str, method: &str) -> Option<wasm_bindgen::JsValue> {
    js_sys::Reflect::get(&js_sys::global(), &wasm_bindgen::JsValue::from_str(object))
        .ok()
        .and_then(|value| {
            js_sys::Reflect::get(&value, &wasm_bindgen::JsValue::from_str(method)).ok()
        })
}

#[cfg(target_arch = "wasm32")]
async fn read_nav_global() -> Option<String> {
    // Lecture du GLOBAL BRUT (chaîne JSON) — le contrat documenté en tête
    // de fichier. Passer par `tour.nav()` renvoie l'objet JS déjà parsé
    // (JSON.parse côté glue) sur lequel `as_string()` échoue → None à
    // chaque tick : la nav hotspot n'a jamais marché sur le web (constat
    // 2026-09-12). Symétrique de `read_move_global`.
    js_sys::Reflect::get(
        &js_sys::global(),
        &wasm_bindgen::JsValue::from_str("__pnexTourLastNav"),
    )
    .ok()?
    .as_string()
}

#[cfg(target_arch = "wasm32")]
async fn read_move_global() -> Option<String> {
    // Global chaîne JSON `__pnexTourHotspotMove` posé par la glue JS.
    read_annot_global("__pnexTourHotspotMove").await
}

#[cfg(target_arch = "wasm32")]
async fn try_call_once(ns: &str, method: &str, args: &[JsArg]) -> bool {
    let Some(f) = global_method("pnexViewers", ns)
        .and_then(|obj| js_sys::Reflect::get(&obj, &wasm_bindgen::JsValue::from_str(method)).ok())
        .and_then(|v| v.dyn_into::<js_sys::Function>().ok())
    else {
        return false;
    };
    let list = js_sys::Array::new();
    for a in args {
        list.push(&match a {
            JsArg::Str(v) => wasm_bindgen::JsValue::from_str(v),
            JsArg::Bool(v) => wasm_bindgen::JsValue::from_bool(*v),
        });
    }
    f.apply(&wasm_bindgen::JsValue::NULL, &list)
        .ok()
        .and_then(|value| value.as_bool())
        .unwrap_or(false)
}

#[cfg(target_arch = "wasm32")]
async fn try_set_tour_editable_once(host_id: &str, on: bool) -> bool {
    let Some(f) = global_method("pnexViewers", "tour")
        .and_then(|tour| {
            js_sys::Reflect::get(&tour, &wasm_bindgen::JsValue::from_str("setTourEditable")).ok()
        })
        .and_then(|v| v.dyn_into::<js_sys::Function>().ok())
    else {
        return false;
    };
    f.call2(
        &wasm_bindgen::JsValue::NULL,
        &wasm_bindgen::JsValue::from_str(host_id),
        &wasm_bindgen::JsValue::from_bool(on),
    )
    .ok()
    .and_then(|value| value.as_bool())
    .unwrap_or(false)
}

/// Lecture du GLOBAL BRUT (chaîne JSON) d'un événement annotation —
/// symétrique de `read_nav_global` (jamais l'objet parsé, bug take_nav
/// 2026-09-12).
#[cfg(target_arch = "wasm32")]
async fn read_annot_global(name: &str) -> Option<String> {
    js_sys::Reflect::get(&js_sys::global(), &wasm_bindgen::JsValue::from_str(name))
        .ok()?
        .as_string()
}

#[cfg(target_arch = "wasm32")]
fn unmount_impl(host_id: &str) {
    if let Some(f) =
        global_method("pnexViewers", "unmount").and_then(|v| v.dyn_into::<js_sys::Function>().ok())
    {
        let _ = f.call1(
            &wasm_bindgen::JsValue::NULL,
            &wasm_bindgen::JsValue::from_str(host_id),
        );
    }
}

#[cfg(target_arch = "wasm32")]
use wasm_bindgen::JsCast;

#[cfg(target_arch = "wasm32")]
async fn sleep_ms(ms: u32) {
    gloo_timers::future::TimeoutFuture::new(ms).await;
}

#[cfg(target_arch = "wasm32")]
fn log_device(_message: &str) {}

// ─────────────────────────── natif (webview) ───────────────────────────

#[cfg(not(target_arch = "wasm32"))]
async fn try_scene_once(method: &str, host_id: &str, scene: &TourSceneView) -> bool {
    let json = serde_json::to_string(scene).unwrap_or_default();
    // Échappement pour l'injection en littéral JS simple-quote.
    let host_q = host_id.replace('\'', "");
    let json_q = json.replace('\\', "\\\\").replace('\'', "\\'");
    eval_bool(format!(
        "return window.pnexViewers?.tour?.{method}('{host_q}', JSON.parse('{json_q}')) === true"
    ))
    .await
}

/// Eval JS → bool (webview native — école tron.rs / media_viewer.rs).
#[cfg(not(target_arch = "wasm32"))]
async fn eval_bool(js: String) -> bool {
    dioxus::document::eval(&js)
        .await
        .ok()
        .and_then(|value| value.as_bool())
        .unwrap_or(false)
}

#[cfg(not(target_arch = "wasm32"))]
async fn read_nav_global() -> Option<String> {
    dioxus::document::eval("return window.__pnexTourLastNav ?? null")
        .await
        .ok()?
        .as_str()
        .map(str::to_string)
}

#[cfg(not(target_arch = "wasm32"))]
async fn read_move_global() -> Option<String> {
    read_annot_global("__pnexTourHotspotMove").await
}

// Arguments are injected as JSON literals (strings stay strings: the glue
// parses the marker JSON itself — passing `JSON.parse(…)` here made it
// parse an array again and fail).
#[cfg(not(target_arch = "wasm32"))]
async fn try_call_once(ns: &str, method: &str, args: &[JsArg]) -> bool {
    let list: Vec<String> = args
        .iter()
        .map(|a| match a {
            JsArg::Str(v) => serde_json::to_string(v).unwrap_or_else(|_| "\"\"".into()),
            JsArg::Bool(v) => v.to_string(),
        })
        .collect();
    eval_bool(format!(
        "return window.pnexViewers?.{ns}?.{method}?.({}) === true",
        list.join(", ")
    ))
    .await
}

#[cfg(not(target_arch = "wasm32"))]
async fn try_set_tour_editable_once(host_id: &str, on: bool) -> bool {
    let host_q = host_id.replace('\'', "");
    eval_bool(format!(
        "return window.pnexViewers?.tour?.setTourEditable('{host_q}', {on}) === true"
    ))
    .await
}

/// Lecture du global brut (chaîne JSON) d'un événement annotation.
#[cfg(not(target_arch = "wasm32"))]
async fn read_annot_global(name: &str) -> Option<String> {
    let name_q = name.replace('\'', "");
    dioxus::document::eval(&format!("return window.{name_q} ?? null"))
        .await
        .ok()?
        .as_str()
        .map(str::to_string)
}

#[cfg(not(target_arch = "wasm32"))]
fn unmount_impl(host_id: &str) {
    let host_q = host_id.replace('\'', "");
    dioxus::prelude::spawn(async move {
        let _ = dioxus::document::eval(&format!("window.pnexViewers?.unmount('{host_q}')")).await;
    });
}

#[cfg(not(target_arch = "wasm32"))]
async fn sleep_ms(ms: u32) {
    futures_timer::Delay::new(std::time::Duration::from_millis(ms as u64)).await;
}
