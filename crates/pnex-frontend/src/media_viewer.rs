//! Pont vers les viewers WebGL de la bibliothèque média (D21) —
//! `assets/viewers.js` (bundle esbuild IIFE de pannellum WebGL1 (panoramas
//! 360° équirect) et gsplat.js WebGL2 (splats .splat/.ply), chargé dans
//! `main.rs` — pattern tron-gerbe.js).
//!
//! Le module JS module : `window.pnexViewers` :
//!   - `panorama.mount(hostId, url)` → bool : pannellum dans le div hôte ;
//!   - `panorama.unmount(hostId)` ;
//!   - `splat.mount(hostId, url)` → bool : canvas WebGL2 gsplat.js ;
//!   - `splat.unmount(hostId)`.
//!
//! Échecs silencieux (script absent, hôte introuvable, WebGL manquant) →
//! `false` : la page affiche le badge « aperçu indisponible » — jamais de
//! panique UI (école tron.rs). Cible non-wasm32 : la webview native exécute
//! le même JS via `document::eval` (prouvé par tron.rs) — l'aperçu marche
//! donc aussi sur Android ; garde `task check` natif vert.

/// Monte le viewer `kind` (`panorama` | `splat`) dans le div `host_id`.
/// Retry 100 ms × 50 (le script peut finir de charger après le premier
/// rendu — école tron.rs) ; `true` si monté.
pub async fn mount(kind: &str, host_id: &str, url: &str) -> bool {
    mount_impl(kind, host_id, url).await
}

/// Updates the WebSocket URL a mounted camera viewer uses on its next
/// reconnection (fresh access token) — fire-and-forget.
pub fn camera_set_url(host_id: &str, url: &str) {
    camera_set_url_impl(host_id, url);
}

/// Démonte le viewer du hôte — fire-and-forget.
pub fn unmount(host_id: &str) {
    unmount_impl(host_id);
}

#[cfg(target_arch = "wasm32")]
async fn mount_impl(kind: &str, host_id: &str, url: &str) -> bool {
    for _ in 0..50 {
        if try_mount(kind, host_id, url) {
            return true;
        }
        gloo_timers::future::TimeoutFuture::new(100).await;
    }
    false
}

/// Un essai de mount — true si le viewer tourne.
#[cfg(target_arch = "wasm32")]
fn try_mount(kind: &str, host_id: &str, url: &str) -> bool {
    global_method("pnexViewers", kind)
        .and_then(|viewer| js_sys::Reflect::get(&viewer, &JsValue::from_str("mount")).ok())
        .and_then(|f| f.dyn_into::<js_sys::Function>().ok())
        .and_then(|f| {
            f.call2(
                &JsValue::NULL,
                &JsValue::from_str(host_id),
                &JsValue::from_str(url),
            )
            .ok()
        })
        .and_then(|value| value.as_bool())
        .unwrap_or(false)
}

/// Résout `window.<objet>.<méthode>` (globales posées par viewers.js).
#[cfg(target_arch = "wasm32")]
fn global_method(object: &str, method: &str) -> Option<JsValue> {
    js_sys::Reflect::get(&js_sys::global(), &JsValue::from_str(object))
        .ok()
        .and_then(|value| js_sys::Reflect::get(&value, &JsValue::from_str(method)).ok())
}

/// Import wasm-bindgen prélué (JsValue + JsCast).
#[cfg(target_arch = "wasm32")]
use wasm_bindgen::{JsCast, JsValue};

#[cfg(target_arch = "wasm32")]
fn unmount_impl(host_id: &str) {
    if let Some(f) =
        global_method("pnexViewers", "unmount").and_then(|v| v.dyn_into::<js_sys::Function>().ok())
    {
        let _ = f.call1(&JsValue::NULL, &JsValue::from_str(host_id));
    }
}

#[cfg(not(target_arch = "wasm32"))]
async fn mount_impl(kind: &str, host_id: &str, url: &str) -> bool {
    // Échappement des apostrophes pour l'injection dans le JS.
    let host_id_q = host_id.replace('\'', "");
    let url_q = url.replace('\'', "%27");
    // Journal fichier (smoke test device — le logger logcat est muet).
    crate::capture360::filelog::log(&format!(
        "viewer mount {kind} host={host_id_q} url={} o",
        url.len()
    ));
    for _ in 0..50 {
        let mounted = eval_bool(format!(
            "return window.pnexViewers?.{kind}?.mount('{host_id_q}','{url_q}') === true"
        ))
        .await;
        if mounted {
            crate::capture360::filelog::log("viewer mount: ok");
            return true;
        }
        futures_timer::Delay::new(std::time::Duration::from_millis(100)).await;
    }
    crate::capture360::filelog::log("viewer mount: ÉCHEC (50 essais)");
    false
}

#[cfg(not(target_arch = "wasm32"))]
fn unmount_impl(host_id: &str) {
    let host_id_q = host_id.replace('\'', "");
    dioxus::prelude::spawn(async move {
        let _ =
            dioxus::document::eval(&format!("window.pnexViewers?.unmount('{host_id_q}')")).await;
    });
}

/// Eval JS → bool (webview native — école tron.rs natif).
#[cfg(not(target_arch = "wasm32"))]
async fn eval_bool(js: String) -> bool {
    dioxus::document::eval(&js)
        .await
        .ok()
        .and_then(|value| value.as_bool())
        .unwrap_or(false)
}

#[cfg(target_arch = "wasm32")]
fn camera_set_url_impl(host_id: &str, url: &str) {
    if let Some(f) = global_method("pnexViewers", "camera")
        .and_then(|camera| js_sys::Reflect::get(&camera, &JsValue::from_str("setUrl")).ok())
        .and_then(|f| f.dyn_into::<js_sys::Function>().ok())
    {
        let _ = f.call2(
            &JsValue::NULL,
            &JsValue::from_str(host_id),
            &JsValue::from_str(url),
        );
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn camera_set_url_impl(host_id: &str, url: &str) {
    let host_id_q = host_id.replace('\'', "");
    let url_q = url.replace('\'', "%27");
    dioxus::prelude::spawn(async move {
        let _ = dioxus::document::eval(&format!(
            "window.pnexViewers?.camera?.setUrl('{host_id_q}','{url_q}')"
        ))
        .await;
    });
}
