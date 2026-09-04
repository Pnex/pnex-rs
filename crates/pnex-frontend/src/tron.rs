//! Pont vers le fond animé « gerbe de faisceaux » de la page de login
//! (`assets/tron-gerbe.js` — script statique sans dépendance npm, chargé
//! dans `main.rs`, cf. le pattern flasher.js).
//!
//! Le module JS expose `window.pnexTronGerbe` :
//!   - `mount(hostId)` : crée le canvas WebGL2 dans le div hôte et démarre
//!     la boucle RAF ; false si WebGL2 indisponible (le dégradé CSS de
//!     secours, posé en style du div hôte, reste alors visible).
//!   - `unmount()` : stoppe le RAF, retire le listener de resize, supprime
//!     canvas et contexte GL.
//!
//! Cible non-wasm32 : stubs — le fond n'existe que sur le web (garde
//! `task check` natif vert, même logique que flash.rs).

#[cfg(target_arch = "wasm32")]
use wasm_bindgen::{JsCast, JsValue};

/// Monte le fond animé dans le div `host_id`. Échecs silencieux (script
/// absent du build, hôte introuvable, WebGL2 manquant) : le dégradé CSS de
/// secours reste visible dans tous les cas — jamais de panique UI.
///
/// CSR : le `<script>` est posé par le même render que la page login — le
/// premier essai peut donc courir avant l'exécution de tron-gerbe.js
/// (fenêtre de fetch). On retente 100 ms × 50 (5 s max) ; au-delà, le
/// dégradé de secours reste affiché.
#[cfg(target_arch = "wasm32")]
pub fn mount(host_id: &str) {
    if try_mount(host_id) {
        return;
    }
    let host_id = host_id.to_string();
    wasm_bindgen_futures::spawn_local(async move {
        for _ in 0..50 {
            gloo_timers::future::TimeoutFuture::new(100).await;
            if try_mount(&host_id) {
                return;
            }
        }
    });
}

/// Un essai de mount — true si la gerbe tourne (déjà montée ou montée là).
#[cfg(target_arch = "wasm32")]
fn try_mount(host_id: &str) -> bool {
    global_method("pnexTronGerbe", "mount")
        .and_then(|f| f.call1(&JsValue::NULL, &JsValue::from_str(host_id)).ok())
        .and_then(|value| value.as_bool())
        .unwrap_or(false)
}

/// Démonte le fond (navigation hors login) — no-op si déjà démonté.
#[cfg(target_arch = "wasm32")]
pub fn unmount() {
    if let Some(unmount_fn) = global_method("pnexTronGerbe", "unmount") {
        let _ = unmount_fn.call0(&JsValue::NULL);
    }
}

/// Résout `window.<objet>.<méthode>` (globales posées par tron-gerbe.js).
#[cfg(target_arch = "wasm32")]
fn global_method(object: &str, method: &str) -> Option<js_sys::Function> {
    js_sys::Reflect::get(&js_sys::global(), &JsValue::from_str(object))
        .ok()
        .and_then(|value| js_sys::Reflect::get(&value, &JsValue::from_str(method)).ok())
        .and_then(|value| value.dyn_into::<js_sys::Function>().ok())
}

#[cfg(not(target_arch = "wasm32"))]
pub fn mount(_host_id: &str) {}

#[cfg(not(target_arch = "wasm32"))]
pub fn unmount() {}
