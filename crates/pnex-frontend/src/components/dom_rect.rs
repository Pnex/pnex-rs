//! Bounding rect of an element by id, on every target.
//!
//! Web reads the DOM synchronously (`web_sys`). Native (Android / desktop)
//! has no synchronous DOM access: a small JS bridge streams the rect of the
//! element (mount, resize, every pointerdown) into a process-wide cache that
//! gesture handlers read. Without it the flow and dashboard editors had no
//! canvas rect on Android: tapping a node never selected it (no inspector,
//! no drag, no wiring).

#[cfg(not(target_arch = "wasm32"))]
use dioxus::prelude::*;

/// `(left, top, width, height)` in CSS pixels, client coordinates.
pub type Rect = (f64, f64, f64, f64);

#[cfg(target_arch = "wasm32")]
pub fn rect_of(id: &str) -> Option<Rect> {
    let document = web_sys::window()?.document()?;
    let element = document.get_element_by_id(id)?;
    let rect = element.get_bounding_client_rect();
    Some((rect.left(), rect.top(), rect.width(), rect.height()))
}

#[cfg(not(target_arch = "wasm32"))]
static CACHE: std::sync::Mutex<Vec<(String, Rect)>> = std::sync::Mutex::new(Vec::new());

#[cfg(not(target_arch = "wasm32"))]
pub fn rect_of(id: &str) -> Option<Rect> {
    let cache = CACHE.lock().ok()?;
    cache
        .iter()
        .find(|(key, _)| key == id)
        .map(|(_, rect)| *rect)
}

#[cfg(not(target_arch = "wasm32"))]
fn store(id: &str, rect: Rect) {
    if let Ok(mut cache) = CACHE.lock() {
        match cache.iter_mut().find(|(key, _)| key == id) {
            Some(entry) => entry.1 = rect,
            None => cache.push((id.to_string(), rect)),
        }
    }
}

/// Keeps [`rect_of`]`(id)` current for the lifetime of the calling component
/// (no-op on web, where the DOM is read directly).
pub fn use_rect_tracker(id: &'static str) {
    #[cfg(target_arch = "wasm32")]
    let _ = id;
    #[cfg(not(target_arch = "wasm32"))]
    use_future(move || async move {
        let js = format!(
            r#"
let el;
while (!(el = document.getElementById({id:?}))) {{
  await new Promise((r) => setTimeout(r, 50));
}}
const send = () => {{
  const r = el.getBoundingClientRect();
  dioxus.send([r.left, r.top, r.width, r.height]);
}};
send();
new ResizeObserver(send).observe(el);
window.addEventListener('resize', send);
el.addEventListener('pointerdown', send, true);
await new Promise(() => {{}});
"#
        );
        let mut bridge = document::eval(&js);
        while let Ok(rect) = bridge.recv::<Vec<f64>>().await {
            if let [left, top, width, height] = rect.as_slice() {
                store(id, (*left, *top, *width, *height));
            }
        }
    });
}
