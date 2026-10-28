//! Bridge to the animated "beam sheaf" background of the login page
//! (`assets/tron-gerbe.js` — static script without npm dependency, loaded
//! in `main.rs`, see the flasher.js pattern).
//!
//! The JS module exposes `window.pnexTronGerbe`:
//!   - `mount(hostId)`: creates the WebGL2 canvas in the host div and starts
//!     the RAF loop; false if WebGL2 is unavailable (the fallback CSS
//!     gradient, set as the host div style, then stays visible).
//!   - `unmount()`: stops the RAF, removes the resize listener, deletes
//!     the canvas and GL context.
//!
//! Non-wasm32 target: stubs — the background exists only on the web (keeps
//! native `task check` green, same logic as flash.rs).

#[cfg(target_arch = "wasm32")]
use wasm_bindgen::{JsCast, JsValue};

/// Mounts the animated background in the `host_id` div. Silent failures
/// (script missing from the build, host not found, missing WebGL2): the
/// fallback CSS gradient stays visible in every case — never a UI panic.
///
/// CSR: the `<script>` is set by the same render as the login page — the
/// first attempt may therefore run before tron-gerbe.js executes
/// (fetch window). We retry 100 ms × 50 (5 s max); beyond that, the
/// fallback gradient stays shown.
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

/// One mount attempt — true if the sheaf runs (already mounted or mounted now).
#[cfg(target_arch = "wasm32")]
fn try_mount(host_id: &str) -> bool {
    global_method("pnexTronGerbe", "mount")
        .and_then(|f| f.call1(&JsValue::NULL, &JsValue::from_str(host_id)).ok())
        .and_then(|value| value.as_bool())
        .unwrap_or(false)
}

/// Unmounts the background (navigation away from login) — no-op if already unmounted.
#[cfg(target_arch = "wasm32")]
pub fn unmount() {
    if let Some(unmount_fn) = global_method("pnexTronGerbe", "unmount") {
        let _ = unmount_fn.call0(&JsValue::NULL);
    }
}

/// Resolves `window.<object>.<method>` (globals set by tron-gerbe.js).
#[cfg(target_arch = "wasm32")]
fn global_method(object: &str, method: &str) -> Option<js_sys::Function> {
    js_sys::Reflect::get(&js_sys::global(), &JsValue::from_str(object))
        .ok()
        .and_then(|value| js_sys::Reflect::get(&value, &JsValue::from_str(method)).ok())
        .and_then(|value| value.dyn_into::<js_sys::Function>().ok())
}

/// Native target (desktop/Android webview): the webview runs the same JS
/// (`tron-gerbe.js` loaded by main.rs) — bridge via `document::eval`, same
/// contract as the web (false if WebGL2 is missing → fallback CSS gradient).
#[cfg(not(target_arch = "wasm32"))]
pub fn mount(host_id: &str) {
    let host_id = host_id.to_string();
    dioxus::prelude::spawn(async move {
        // Same retry window as the web: the script may finish loading
        // after the first attempt (100 ms × 50 = 5 s max, silent failure).
        for _ in 0..50 {
            let mounted = dioxus::document::eval(&format!(
                "return window.pnexTronGerbe?.mount('{host_id}') === true"
            ))
            .await
            .ok()
            .and_then(|value| value.as_bool())
            .unwrap_or(false);
            if mounted {
                return;
            }
            futures_timer::Delay::new(std::time::Duration::from_millis(100)).await;
        }
    });
}

/// Native unmount — fire-and-forget (no Rust state to free).
#[cfg(not(target_arch = "wasm32"))]
pub fn unmount() {
    dioxus::prelude::spawn(async move {
        let _ = dioxus::document::eval("window.pnexTronGerbe?.unmount()").await;
    });
}
