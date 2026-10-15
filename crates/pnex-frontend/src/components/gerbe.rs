//! The one loading mark (design-system.md D224): a miniature of the login
//! page's « gerbe de faisceaux » — a fan of beams converging on a focus,
//! short segments flowing up into it. Plain SVG + CSS (`.pnex-gerbe` in
//! style/tailwind.css): no WebGL, any number of instances, the motion
//! stops under `prefers-reduced-motion`.

use dioxus::prelude::*;
use dioxus_i18n::t;

/// Beams: x of the foot of each beam (viewBox 40 × 24, focus at 20,2).
const FEET: [f32; 7] = [3.0, 8.5, 14.0, 20.0, 26.0, 31.5, 37.0];

fn beam(x: f32) -> String {
    // Rises straight from its foot, bends into the focus.
    format!("M{x} 23 C{x} 13 20 11 20 2")
}

/// Per beam: its own pace (flow) and its own place in the palette cycle.
fn flow_style(i: usize) -> String {
    let duration = if i % 3 == 2 { 0.7 } else { 0.9 };
    let flow = -(i as f32) * 0.31;
    let hue = -(i as f32) * 0.7;
    format!("animation-duration: {duration}s, 4.2s; animation-delay: {flow}s, {hue}s;")
}

/// Loading mark. `size` = width in px (height follows the 40:24 ratio).
/// `delayed`: hidden for the first 250 ms, so a fast load never flashes;
/// by default a page-level mark (32 px and more) waits, a small one (in a
/// button, a pill) shows at once.
#[component]
pub fn Gerbe(
    #[props(default = 32)] size: u32,
    #[props(default)] delayed: Option<bool>,
    /// Layout classes only (margins, display).
    #[props(default)]
    class: String,
    /// Takes the parent's text colour (on a dark overlay, in a filled
    /// button) instead of the signal colour.
    #[props(default)]
    inherit: bool,
) -> Element {
    let delayed = delayed.unwrap_or(size >= 32);
    let height = size * 24 / 40;
    let label = t!("common-loading").to_string();
    // The login gerbe's palette flows along the beams, unless the mark
    // takes its parent's colour.
    let base = match (delayed, inherit) {
        (true, false) => "pnex-gerbe pnex-gerbe-delayed pnex-gerbe-color text-signal",
        (true, true) => "pnex-gerbe pnex-gerbe-delayed",
        (false, false) => "pnex-gerbe pnex-gerbe-color text-signal",
        (false, true) => "pnex-gerbe",
    };
    rsx! {
        svg {
            class: "{base} {class}",
            width: "{size}",
            height: "{height}",
            view_box: "0 0 40 24",
            fill: "none",
            role: "status",
            "aria-label": "{label}",
            for x in FEET {
                path { key: "t{x}", class: "pnex-gerbe-track", d: beam(x) }
            }
            for (i, x) in FEET.iter().enumerate() {
                path {
                    key: "f{x}",
                    class: "pnex-gerbe-flow",
                    style: flow_style(i),
                    d: beam(*x),
                }
            }
        }
    }
}
