//! Bridge to the WebGL viewers of the media library (D21) —
//! `assets/viewers.js` (esbuild IIFE bundle of pannellum WebGL1 (360°
//! equirect panoramas) and gsplat.js WebGL2 (.splat/.ply splats), loaded in
//! `main.rs` — tron-gerbe.js pattern).
//!
//! The JS module: `window.pnexViewers`:
//!   - `panorama.mount(hostId, url)` → bool: pannellum in the host div;
//!   - `panorama.unmount(hostId)`;
//!   - `splat.mount(hostId, url)` → bool: gsplat.js WebGL2 canvas;
//!   - `splat.unmount(hostId)`.
//!
//! Silent failures (missing script, host not found, missing WebGL) →
//! `false`: the page shows the "preview unavailable" badge — never a
//! UI panic (tron.rs school). Non-wasm32 target: the native webview runs
//! the same JS via `document::eval` (proven by tron.rs) — so the preview
//! also works on Android; keeps native `task check` green.

/// Mounts the `kind` viewer (`panorama` | `splat`) in the `host_id` div.
/// Retry 100 ms × 50 (the script may finish loading after the first
/// render — tron.rs school); `true` if mounted.
pub async fn mount(kind: &str, host_id: &str, url: &str) -> bool {
    mount_impl(kind, host_id, url).await
}

/// Updates the WebSocket URL a mounted camera viewer uses on its next
/// reconnection (fresh access token) — fire-and-forget.
pub fn camera_set_url(host_id: &str, url: &str) {
    camera_set_url_impl(host_id, url);
}

/// Calls `window.pnexViewers.mesh.<method>(host_id, arg)` (3D viewer,
/// `js/mesh.js`) and returns its JSON result; `None` when the viewer is
/// missing or throws. `document::eval` runs on web and in the webview alike.
pub async fn mesh_call(
    host_id: &str,
    method: &'static str,
    arg: serde_json::Value,
) -> Option<serde_json::Value> {
    let host = serde_json::to_string(host_id).ok()?;
    let js = format!("return window.pnexViewers?.mesh?.{method}({host}, {arg}) ?? null");
    dioxus::document::eval(&js)
        .await
        .ok()
        .filter(|v| !v.is_null())
}

/// Unmounts the host's viewer — fire-and-forget.
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

/// One mount attempt — true if the viewer runs.
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

/// Resolves `window.<object>.<method>` (globals set by viewers.js).
#[cfg(target_arch = "wasm32")]
fn global_method(object: &str, method: &str) -> Option<JsValue> {
    js_sys::Reflect::get(&js_sys::global(), &JsValue::from_str(object))
        .ok()
        .and_then(|value| js_sys::Reflect::get(&value, &JsValue::from_str(method)).ok())
}

/// Prelude wasm-bindgen import (JsValue + JsCast).
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
    // Escape apostrophes for injection into the JS.
    let host_id_q = host_id.replace('\'', "");
    let url_q = url.replace('\'', "%27");
    // File log (device smoke test — the logcat logger is silent).
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
    crate::capture360::filelog::log("viewer mount: FAILED (50 attempts)");
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

/// JS eval → bool (native webview — native tron.rs school).
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
