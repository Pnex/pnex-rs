//! Local graph of an object (D186, lot L4 "vue graphe Dioxus"): the object
//! in the centre, its neighbours on a ring, their neighbours on an outer
//! ring. Plain SVG (no layout engine): a neighbourhood is small and a
//! radial layout reads well. Clicking a node opens its object page.

use std::collections::BTreeMap;

use dioxus::prelude::*;
use pnex_core::ontology::api::{LinkTypeView, Neighborhood};

use crate::app::Route;

const W: f64 = 640.0;
const H: f64 = 420.0;
const R1: f64 = 130.0;
const R2: f64 = 195.0;

/// Ring of each node: 0 for the centre, 1 for direct neighbours, 2 beyond.
pub fn rings(n: &Neighborhood, center: &str) -> BTreeMap<String, u8> {
    let mut ring = BTreeMap::new();
    ring.insert(center.to_string(), 0u8);
    for l in &n.links {
        for (a, b) in [(&l.source.id, &l.target.id), (&l.target.id, &l.source.id)] {
            if a == center {
                ring.entry(b.clone()).or_insert(1);
            }
        }
    }
    for node in &n.nodes {
        ring.entry(node.id.clone()).or_insert(2);
    }
    ring
}

/// Positions on concentric rings, evenly spaced by ring.
pub fn layout(n: &Neighborhood, center: &str) -> BTreeMap<String, (f64, f64)> {
    let rings = rings(n, center);
    let mut by_ring: BTreeMap<u8, Vec<String>> = BTreeMap::new();
    for node in &n.nodes {
        by_ring
            .entry(rings[&node.id])
            .or_default()
            .push(node.id.clone());
    }
    let mut pos = BTreeMap::new();
    for (ring, ids) in by_ring {
        let r = match ring {
            0 => 0.0,
            1 => R1,
            _ => R2,
        };
        let count = ids.len().max(1) as f64;
        // The outer ring is turned half a step so its nodes fall between.
        let phase = if ring == 2 {
            std::f64::consts::PI / count
        } else {
            -std::f64::consts::FRAC_PI_2
        };
        for (i, id) in ids.into_iter().enumerate() {
            let a = phase + i as f64 * std::f64::consts::TAU / count;
            pos.insert(id, (W / 2.0 + r * a.cos(), H / 2.0 + r * a.sin()));
        }
    }
    pos
}

#[component]
pub fn GraphView(
    neighborhood: Neighborhood,
    center: String,
    link_types: Vec<LinkTypeView>,
) -> Element {
    let pos = layout(&neighborhood, &center);
    let edges: Vec<(String, (f64, f64), (f64, f64), String)> = neighborhood
        .links
        .iter()
        .filter_map(|l| {
            let a = *pos.get(&l.source.id)?;
            let b = *pos.get(&l.target.id)?;
            let def = link_types
                .iter()
                .find(|t| t.def.key == l.link_type)
                .map(|t| &t.def);
            Some((
                l.id.to_string(),
                a,
                b,
                super::link_label(def, &l.link_type, false),
            ))
        })
        .collect();
    let nodes: Vec<(String, String, String, (f64, f64), bool)> = neighborhood
        .nodes
        .iter()
        .filter_map(|n| {
            let p = *pos.get(&n.id)?;
            Some((
                n.id.clone(),
                n.title.clone(),
                super::key_label(&n.type_key),
                p,
                n.id == center,
            ))
        })
        .collect();
    let navigator = use_navigator();
    rsx! {
        svg {
            class: "w-full h-auto bg-gray-50 rounded-lg border border-gray-200",
            view_box: "0 0 {W} {H}",
            "data-testid": "ontology-graph",
            defs {
                marker {
                    id: "onto-arrow",
                    view_box: "0 0 10 10",
                    ref_x: "28",
                    ref_y: "5",
                    marker_width: "6",
                    marker_height: "6",
                    orient: "auto-start-reverse",
                    path { d: "M 0 0 L 10 5 L 0 10 z", fill: "#9ca3af" }
                }
            }
            for (id, (x1, y1), (x2, y2), label) in edges {
                g { key: "e{id}",
                    line {
                        x1: "{x1}",
                        y1: "{y1}",
                        x2: "{x2}",
                        y2: "{y2}",
                        stroke: "#9ca3af",
                        stroke_width: "1.5",
                        marker_end: "url(#onto-arrow)",
                    }
                    text {
                        x: "{(x1 + x2) / 2.0}",
                        y: "{(y1 + y2) / 2.0 - 4.0}",
                        text_anchor: "middle",
                        font_size: "10",
                        fill: "#6b7280",
                        "{label}"
                    }
                }
            }
            for (id, title, kind, (x, y), is_center) in nodes {
                g {
                    key: "n{id}",
                    class: "cursor-pointer",
                    onclick: move |_| {
                        navigator
                            .push(Route::OntologyObject {
                                id: id.clone(),
                            });
                    },
                    circle {
                        cx: "{x}",
                        cy: "{y}",
                        r: if is_center { "22" } else { "16" },
                        fill: if is_center { "#2563eb" } else { "#ffffff" },
                        stroke: "#2563eb",
                        stroke_width: "2",
                    }
                    text {
                        x: "{x}",
                        y: "{y + 34.0}",
                        text_anchor: "middle",
                        font_size: "12",
                        fill: "#111827",
                        "{title}"
                    }
                    text {
                        x: "{x}",
                        y: "{y + 47.0}",
                        text_anchor: "middle",
                        font_size: "10",
                        fill: "#6b7280",
                        "{kind}"
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pnex_core::ontology::api::{LinkView, ObjectRef};

    fn r(id: &str) -> ObjectRef {
        ObjectRef {
            id: id.into(),
            type_key: "pump".into(),
            title: id.into(),
            native_id: id.into(),
        }
    }

    fn l(id: i64, a: &str, b: &str) -> LinkView {
        LinkView {
            id,
            link_type: "feeds".into(),
            source: r(a),
            target: r(b),
            attributes: Default::default(),
            valid_from: String::new(),
            valid_to: None,
            source_ref: None,
        }
    }

    #[test]
    fn rings_and_positions() {
        let n = Neighborhood {
            nodes: ["c", "a", "b", "far"].map(r).to_vec(),
            links: vec![l(1, "c", "a"), l(2, "b", "c"), l(3, "a", "far")],
        };
        let rings = rings(&n, "c");
        assert_eq!(
            (rings["c"], rings["a"], rings["b"], rings["far"]),
            (0, 1, 1, 2)
        );
        let pos = layout(&n, "c");
        assert_eq!(pos["c"], (W / 2.0, H / 2.0));
        let (x, y) = pos["a"];
        let d = ((x - W / 2.0).powi(2) + (y - H / 2.0).powi(2)).sqrt();
        assert!((d - R1).abs() < 1e-6);
    }
}
