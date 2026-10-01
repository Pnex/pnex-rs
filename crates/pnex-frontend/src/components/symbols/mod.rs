//! Process / flowchart symbol library of the SCADA dashboards (`symbol`
//! widget). Original PneX drawings following the usual conventions
//! (ISO 10628 / ISA 5.1 for process, ISO 5807 for flowcharts), one
//! hand-written file per category. Each entry is an inline SVG fragment
//! drawn in `0..w × 0..h`, whose default fill / stroke are inherited from
//! the root `<g>` rendered here (`fill="none"` = line only,
//! `currentColor` = solid detail in the stroke colour).

mod lib_flowchart;
mod lib_handling;
mod lib_heat;
mod lib_instruments;
mod lib_mixing;
mod lib_piping;
mod lib_rotating;
mod lib_separation;
mod lib_valves;
mod lib_vessels;

use dioxus::prelude::*;
use dioxus_i18n::prelude::*;

/// Categories in palette order (fluent keys `sym-cat-<key>`, symbol names
/// `sym-<id>`).
pub static LIBRARY: &[(&str, &[Symbol])] = &[
    ("flowchart", lib_flowchart::SYMBOLS),
    ("valves", lib_valves::SYMBOLS),
    ("rotating", lib_rotating::SYMBOLS),
    ("vessels", lib_vessels::SYMBOLS),
    ("heat", lib_heat::SYMBOLS),
    ("instruments", lib_instruments::SYMBOLS),
    ("piping", lib_piping::SYMBOLS),
    ("mixing", lib_mixing::SYMBOLS),
    ("separation", lib_separation::SYMBOLS),
    ("handling", lib_handling::SYMBOLS),
];

/// Every symbol, category order.
pub fn all() -> impl Iterator<Item = &'static Symbol> {
    LIBRARY.iter().flat_map(|(_, list)| list.iter())
}

/// One catalog entry (static data, generated).
#[derive(Debug, PartialEq, Eq)]
pub struct Symbol {
    pub id: &'static str,
    pub category: &'static str,
    /// Canonical English name (fallback when a `sym-<id>` key is missing).
    pub name: &'static str,
    pub w: u16,
    pub h: u16,
    pub body: &'static str,
}

/// Default stroke of a symbol (dark gray, readable on the light canvas).
pub const DEFAULT_STROKE: &str = "#1f2937";
/// Default fill (white, like draw.io).
pub const DEFAULT_FILL: &str = "#ffffff";

/// Catalog lookup by id.
pub fn find(id: &str) -> Option<&'static Symbol> {
    all().find(|s| s.id == id)
}

/// Localized symbol name — render scope only (uses the i18n context).
/// Missing keys fall back to the canonical English name, never panic.
pub fn symbol_name(sym: &Symbol) -> String {
    i18n()
        .try_translate(&format!("sym-{}", sym.id))
        .unwrap_or_else(|_| sym.name.to_string())
}

/// Localized category label — render scope only.
pub fn category_label(category: &str) -> String {
    i18n()
        .try_translate(&format!("sym-cat-{category}"))
        .unwrap_or_else(|_| category.to_string())
}

/// Default widget size for a symbol: longest side 80 px, aspect kept,
/// each side at least `WIDGET_MIN` (thin symbols get a padded box).
pub fn default_size(id: &str) -> (i64, i64) {
    let min = pnex_core::WIDGET_MIN;
    let Some(sym) = find(id) else {
        return (80, 80);
    };
    let (w, h) = (sym.w.max(1) as f64, sym.h.max(1) as f64);
    let k = 80.0 / w.max(h);
    let size = |v: f64| ((v * k / 10.0).round() as i64 * 10).max(min);
    (size(w), size(h))
}

/// SVG `viewBox` + `transform` of the rotated / mirrored symbol — pure.
/// Rotation is clockwise around the drawing, the mirror is applied first
/// (in the symbol's own frame); 90/270 swap the viewBox sides.
pub fn view_geometry(w: u16, h: u16, rotation: u16, flip: bool) -> (String, String) {
    let (w, h) = (w as f64, h as f64);
    let (vw, vh, rotate) = match rotation {
        90 => (h, w, format!("translate({h} 0) rotate(90)")),
        180 => (w, h, format!("translate({w} {h}) rotate(180)")),
        270 => (h, w, format!("translate(0 {w}) rotate(270)")),
        _ => (w, h, String::new()),
    };
    let mirror = if flip {
        format!("translate({w} 0) scale(-1 1)")
    } else {
        String::new()
    };
    let transform = [rotate, mirror]
        .into_iter()
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join(" ");
    (format!("0 0 {vw} {vh}"), transform)
}

/// Renders a catalog symbol. Unknown ids draw a dashed placeholder box
/// (open enum: a layout saved by a newer front end stays displayable).
#[component]
pub fn SymbolView(
    shape: String,
    #[props(default)] stroke: Option<String>,
    #[props(default)] fill: Option<String>,
    #[props(default)] rotation: u16,
    #[props(default)] flip: bool,
    #[props(default)] stretch: bool,
    /// Stroke width in screen px (non-scaling).
    #[props(default = 1.5)]
    stroke_width: f64,
    #[props(default)] class: Option<String>,
) -> Element {
    let stroke = stroke.unwrap_or_else(|| DEFAULT_STROKE.to_string());
    let fill = fill.unwrap_or_else(|| DEFAULT_FILL.to_string());
    let class = class.unwrap_or_else(|| "h-full w-full".to_string());
    let aspect = if stretch { "none" } else { "xMidYMid meet" };
    let Some(sym) = find(&shape) else {
        return rsx! {
            svg {
                class: "pnex-sym {class}",
                view_box: "0 0 100 100",
                "preserveAspectRatio": "{aspect}",
                rect { x: "2", y: "2", width: "96", height: "96", fill: "none",
                    stroke: "{stroke}", "stroke-dasharray": "6 4", "stroke-width": "{stroke_width}" }
            }
        };
    };
    let (view_box, transform) = view_geometry(sym.w, sym.h, rotation, flip);
    rsx! {
        svg {
            class: "pnex-sym {class}",
            xmlns: "http://www.w3.org/2000/svg",
            view_box: "{view_box}",
            "preserveAspectRatio": "{aspect}",
            overflow: "visible",
            g {
                transform: "{transform}",
                fill: "{fill}",
                stroke: "{stroke}",
                color: "{stroke}",
                "stroke-width": "{stroke_width}",
                "stroke-linejoin": "round",
                dangerous_inner_html: sym.body,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn catalog_ids_are_unique_valid_and_categorized() {
        let mut seen = HashSet::new();
        for (cat, list) in LIBRARY {
            assert!(!list.is_empty(), "{cat} is empty");
            for s in list.iter() {
                assert!(pnex_core::valid_symbol_id(s.id), "{}", s.id);
                assert!(seen.insert(s.id), "duplicate {}", s.id);
                assert_eq!(s.category, *cat, "{}", s.id);
                assert!(s.id.starts_with(&format!("{cat}-")), "{}", s.id);
                assert!(!s.body.is_empty(), "{} has no drawing", s.id);
                assert!(!s.body.contains("<script"), "{}", s.id);
                assert!(!s.body.contains(" on"), "{} has an event attribute", s.id);
            }
        }
    }

    #[test]
    fn default_size_keeps_aspect_and_minimum() {
        for s in all() {
            let (w, h) = default_size(s.id);
            assert!(
                w >= pnex_core::WIDGET_MIN && h >= pnex_core::WIDGET_MIN,
                "{}",
                s.id
            );
            assert!(w <= 80 && h <= 80, "{}", s.id);
        }
        assert_eq!(default_size("unknown-shape"), (80, 80));
    }

    #[test]
    fn view_geometry_rotation_swaps_sides() {
        assert_eq!(
            view_geometry(100, 50, 0, false),
            ("0 0 100 50".into(), String::new())
        );
        let (vb, tr) = view_geometry(100, 50, 90, false);
        assert_eq!(vb, "0 0 50 100");
        assert_eq!(tr, "translate(50 0) rotate(90)");
        let (vb, tr) = view_geometry(100, 50, 270, true);
        assert_eq!(vb, "0 0 50 100");
        assert_eq!(
            tr,
            "translate(0 100) rotate(270) translate(100 0) scale(-1 1)"
        );
    }
}
