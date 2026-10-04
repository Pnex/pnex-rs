//! Two-finger pinch on the flow canvas (phones, tablets).
//!
//! Dioxus pointer handlers only see one pointer at a time and the node
//! handlers stop propagation, so the second finger is tracked by a small JS
//! bridge listening in the **capture** phase on `#flow-canvas` (runs the same
//! on web and in the native WebView). Once two touch pointers are down the
//! bridge owns the gesture: it swallows the touch pointer events (the Rust
//! pan / drag / wiring never sees them) until every finger is lifted, and
//! streams `[1, scale, mid_x, mid_y, dx, dy]` frames (midpoint relative to
//! the canvas). `[0]` marks the start of a pinch: the gesture the first
//! finger began is dropped. The bridge also cancels the click that follows a
//! tap on the canvas (ghost click on the freshly opened inspector).

use super::*;

const PINCH_BRIDGE_JS: &str = r#"
let el;
while (!(el = document.getElementById('flow-canvas'))) {
  await new Promise((r) => setTimeout(r, 100));
}
const pts = new Map();
let pinching = false;
let last = null;
const geo = () => {
  const [a, b] = [...pts.values()];
  const r = el.getBoundingClientRect();
  return { d: Math.hypot(a.x - b.x, a.y - b.y), mx: (a.x + b.x) / 2 - r.left, my: (a.y + b.y) / 2 - r.top };
};
const swallow = (e) => { e.stopPropagation(); e.preventDefault(); };
el.addEventListener('pointerdown', (e) => {
  if (e.pointerType !== 'touch') return;
  pts.set(e.pointerId, { x: e.clientX, y: e.clientY });
  if (pts.size >= 2) {
    if (!pinching) dioxus.send([0]);
    pinching = true;
    last = geo();
  }
  if (pinching) swallow(e);
}, true);
el.addEventListener('pointermove', (e) => {
  if (!pts.has(e.pointerId)) return;
  pts.set(e.pointerId, { x: e.clientX, y: e.clientY });
  if (!pinching) return;
  swallow(e);
  if (pts.size < 2 || !last) return;
  const g = geo();
  if (last.d > 0 && g.d > 0) {
    dioxus.send([1, g.d / last.d, g.mx, g.my, g.mx - last.mx, g.my - last.my]);
  }
  last = g;
}, true);
const up = (e) => {
  if (!pts.has(e.pointerId)) return;
  pts.delete(e.pointerId);
  if (!pinching) return;
  swallow(e);
  if (pts.size === 0) pinching = false;
  last = pts.size >= 2 ? geo() : null;
};
// No compatibility mouse events / click after a touch: the click of a tap
// that selects a node landed on the inspector that had just opened under the
// finger (focused field, keyboard popping up).
el.addEventListener('touchstart', (e) => e.preventDefault(), { passive: false });
el.addEventListener('pointerup', up, true);
el.addEventListener('pointercancel', up, true);
await new Promise(() => {});
"#;

/// Installs the pinch bridge for the lifetime of the canvas.
pub(super) fn use_pinch_zoom(mut cx: EditorCx) {
    use_future(move || async move {
        let mut bridge = document::eval(PINCH_BRIDGE_JS);
        while let Ok(frame) = bridge.recv::<Vec<f64>>().await {
            match frame.as_slice() {
                [kind, ..] if *kind == 0.0 => cx.interaction.set(Interaction::Idle),
                [_, scale, mx, my, dx, dy] => {
                    let old = cx.zoom.cloned();
                    let new = (old * scale).clamp(geometry::ZOOM_MIN, geometry::ZOOM_MAX);
                    let pan = geometry::zoom_pan_towards(cx.pan.cloned(), old, new, (*mx, *my));
                    cx.pan.set((pan.0 + dx, pan.1 + dy));
                    cx.zoom.set(new);
                }
                _ => {}
            }
        }
    });
}
