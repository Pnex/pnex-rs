//! Bridge to the Sigma case graph (cases-graph-sigma.md GS1–GS2):
//! `assets/graph.js` exposes `window.pnexGraph`, loaded by `main.rs` like
//! `map.js`. Every payload crosses as JSON (R13). Events come back by
//! polling `window.__pnexGraphEvent` (no closure crosses the bridge, works
//! on wasm and native alike, same school as `map_viewer.rs`). Failures
//! (stub bundle, no WebGL, no host) answer `false`: the caller falls back
//! to the SVG graph, never a panic.

use std::collections::HashMap;

use serde::Deserialize;
use serde_json::Value;

/// The last gesture reported by graph.js.
#[derive(Debug, Deserialize)]
pub struct GraphEvent {
    pub seq: u64,
    pub host: String,
    /// `click` | `dblclick` | `stage` | `drop`.
    pub kind: String,
    pub id: Option<String>,
}

/// Mounts the renderer in `host_id`, retrying while the bundle loads (100 ms
/// × 50); gives up at once when the browser has no WebGL.
pub async fn mount(host_id: &str, opts: &Value) -> bool {
    let opts = opts.to_string();
    for _ in 0..50 {
        if invoke("mount", host_id, Some(&opts)).await == Some(Value::Bool(true)) {
            return true;
        }
        if error(host_id).await.as_deref() == Some("webgl") {
            return false;
        }
        crate::util::sleep(std::time::Duration::from_millis(100)).await;
    }
    false
}

/// `setData`, `setState`: one JSON argument; `true` when applied.
pub async fn send(host_id: &str, method: &str, payload: &Value) -> bool {
    invoke(method, host_id, Some(&payload.to_string())).await == Some(Value::Bool(true))
}

/// `relayout`, `fit`, `zoomIn`, `zoomOut`: no argument.
pub async fn act(host_id: &str, method: &str) -> bool {
    invoke(method, host_id, None).await == Some(Value::Bool(true))
}

/// Force-view positions, for saving the arrangement.
pub async fn positions(host_id: &str) -> HashMap<String, (f64, f64)> {
    let Some(Value::String(raw)) = invoke("positions", host_id, None).await else {
        return HashMap::new();
    };
    let parsed: HashMap<String, [f64; 2]> = serde_json::from_str(&raw).unwrap_or_default();
    parsed.into_iter().map(|(k, [x, y])| (k, (x, y))).collect()
}

/// Why the last mount failed (`lib` | `webgl` | `host`), if it did.
pub async fn error(host_id: &str) -> Option<String> {
    match invoke("error", host_id, None).await {
        Some(Value::String(e)) if !e.is_empty() => Some(e),
        // No pnexGraph at all: the bundle is missing (stub).
        None | Some(Value::Null) => Some("lib".into()),
        _ => None,
    }
}

/// Fire and forget; `spawn_forever` because a task spawned from `use_drop`
/// dies with its scope before it runs.
pub fn unmount(host_id: &str) {
    let host_id = host_id.to_string();
    dioxus::dioxus_core::spawn_forever(async move {
        let _ = invoke("unmount", &host_id, None).await;
    });
}

/// The last gesture on `host_id` newer than `prev_seq`.
pub async fn take_event(host_id: &str, prev_seq: u64) -> Option<GraphEvent> {
    let raw = last_event().await?;
    let ev: GraphEvent = serde_json::from_str(&raw).ok()?;
    (ev.seq > prev_seq && ev.host == host_id).then_some(ev)
}

#[cfg(target_arch = "wasm32")]
async fn invoke(method: &str, host_id: &str, arg: Option<&str>) -> Option<Value> {
    use wasm_bindgen::{JsCast, JsValue};
    let global = js_sys::global();
    let object = js_sys::Reflect::get(&global, &JsValue::from_str("pnexGraph")).ok()?;
    if object.is_undefined() {
        return None;
    }
    let f = js_sys::Reflect::get(&object, &JsValue::from_str(method))
        .ok()?
        .dyn_into::<js_sys::Function>()
        .ok()?;
    let args = js_sys::Array::new();
    args.push(&JsValue::from_str(host_id));
    if let Some(a) = arg {
        args.push(&JsValue::from_str(a));
    }
    let out = f.apply(&JsValue::NULL, &args).ok()?;
    Some(if let Some(b) = out.as_bool() {
        Value::Bool(b)
    } else if let Some(s) = out.as_string() {
        Value::String(s)
    } else {
        Value::Null
    })
}

#[cfg(target_arch = "wasm32")]
async fn last_event() -> Option<String> {
    js_sys::Reflect::get(
        &js_sys::global(),
        &wasm_bindgen::JsValue::from_str("__pnexGraphEvent"),
    )
    .ok()?
    .as_string()
}

#[cfg(not(target_arch = "wasm32"))]
async fn invoke(method: &str, host_id: &str, arg: Option<&str>) -> Option<Value> {
    // Strings injected into JS source are escaped (media_viewer.rs school).
    let host_q = host_id.replace(['\'', '\\'], "");
    let arg_q = arg
        .map(|a| format!(", '{}'", a.replace('\\', "\\\\").replace('\'', "\\'")))
        .unwrap_or_default();
    let js = format!(
        "const g = window.pnexGraph; if (!g) return null; \
         const r = g.{method}('{host_q}'{arg_q}); return r === undefined ? false : r"
    );
    let out = dioxus::document::eval(&js).await.ok()?;
    match out {
        Value::Null => None,
        v => Some(v),
    }
}

#[cfg(not(target_arch = "wasm32"))]
async fn last_event() -> Option<String> {
    dioxus::document::eval("return window.__pnexGraphEvent || null")
        .await
        .ok()?
        .as_str()
        .map(str::to_string)
}
