//! Graph of tables, left to right: what a pipeline reads and writes, or what a table
//! reads and what reads it. Plain SVG with a layered layout: these graphs are small and
//! read in one direction, which the force view of the case graph does not show.

use std::collections::{BTreeMap, BTreeSet};

use dioxus::prelude::*;
use pnex_core::pipelines::PipelineDocument;

const NODE_W: f64 = 190.0;
const NODE_H: f64 = 54.0;
const GAP_X: f64 = 70.0;
const GAP_Y: f64 = 20.0;
const NAME_CHARS: usize = 22;

#[derive(Debug, Clone, PartialEq)]
pub struct GraphNode {
    /// Table name: the key of edges and selection.
    pub id: String,
    /// Under the name: rows, or why there are none.
    pub detail: String,
    /// Marked in the alarm colour.
    pub warn: bool,
}

/// Column of each node: the longest path from a node nothing feeds. A cycle cannot come
/// from a valid document; its nodes stop growing at the node count.
pub fn columns(nodes: &[GraphNode], edges: &[(String, String)]) -> BTreeMap<String, usize> {
    let mut col: BTreeMap<String, usize> = nodes.iter().map(|n| (n.id.clone(), 0)).collect();
    for _ in 0..nodes.len() {
        for (from, to) in edges {
            let next = col.get(from).copied().unwrap_or(0) + 1;
            if next < nodes.len() && col.get(to).is_some_and(|c| *c < next) {
                col.insert(to.clone(), next);
            }
        }
    }
    col
}

/// Top-left corner of each node, and the size of the whole.
pub fn layout(
    nodes: &[GraphNode],
    edges: &[(String, String)],
) -> (BTreeMap<String, (f64, f64)>, (f64, f64)) {
    let col = columns(nodes, edges);
    let mut used: BTreeMap<usize, usize> = BTreeMap::new();
    let mut pos = BTreeMap::new();
    for n in nodes {
        let c = col.get(&n.id).copied().unwrap_or(0);
        let row = used.entry(c).or_default();
        pos.insert(
            n.id.clone(),
            (
                c as f64 * (NODE_W + GAP_X) + GAP_Y,
                *row as f64 * (NODE_H + GAP_Y) + GAP_Y,
            ),
        );
        *row += 1;
    }
    let width = pos.values().map(|p| p.0).fold(0.0, f64::max) + NODE_W + GAP_Y;
    let height = pos.values().map(|p| p.1).fold(0.0, f64::max) + NODE_H + GAP_Y;
    (pos, (width, height))
}

/// Tables of a document, in reading order, and who feeds whom.
pub fn document_graph(doc: &PipelineDocument) -> (Vec<String>, Vec<(String, String)>) {
    let mut seen = BTreeSet::new();
    let mut names = Vec::new();
    let mut edges = Vec::new();
    let mut add = |name: &str, names: &mut Vec<String>| {
        if seen.insert(name.to_string()) {
            names.push(name.to_string());
        }
    };
    for t in &doc.transforms {
        for table in t.inputs.values() {
            add(table, &mut names);
            edges.push((table.clone(), t.output.clone()));
        }
        add(&t.output, &mut names);
    }
    (names, edges)
}

fn short(name: &str) -> String {
    if name.chars().count() > NAME_CHARS {
        let kept: String = name.chars().take(NAME_CHARS - 1).collect();
        format!("{kept}…")
    } else {
        name.to_string()
    }
}

#[component]
pub fn TableGraph(
    nodes: Vec<GraphNode>,
    edges: Vec<(String, String)>,
    #[props(default)] selected: Option<String>,
    on_select: EventHandler<String>,
) -> Element {
    let (pos, (width, height)) = layout(&nodes, &edges);
    let lines: Vec<(String, f64, f64, f64, f64)> = edges
        .iter()
        .filter_map(|(a, b)| {
            let from = pos.get(a)?;
            let to = pos.get(b)?;
            Some((
                format!("{a}>{b}"),
                from.0 + NODE_W,
                from.1 + NODE_H / 2.0,
                to.0,
                to.1 + NODE_H / 2.0,
            ))
        })
        .collect();
    rsx! {
        svg {
            class: "max-w-none",
            width: "{width}",
            height: "{height}",
            view_box: "0 0 {width} {height}",
            "data-testid": "table-graph",
            defs {
                marker {
                    id: "table-arrow",
                    view_box: "0 0 10 10",
                    ref_x: "9",
                    ref_y: "5",
                    marker_width: "6",
                    marker_height: "6",
                    orient: "auto",
                    path {
                        d: "M 0 0 L 10 5 L 0 10 z",
                        fill: "var(--color-gray-400, #9ca3af)",
                    }
                }
            }
            for (key, x1, y1, x2, y2) in lines {
                path {
                    key: "{key}",
                    d: "M {x1} {y1} C {x1 + GAP_X / 2.0} {y1}, {x2 - GAP_X / 2.0} {y2}, {x2} {y2}",
                    fill: "none",
                    stroke: "var(--color-gray-400, #9ca3af)",
                    stroke_width: "1.5",
                    marker_end: "url(#table-arrow)",
                }
            }
            for n in nodes {
                {
                    let (x, y) = pos.get(&n.id).copied().unwrap_or_default();
                    let active = selected.as_deref() == Some(n.id.as_str());
                    let stroke = if active {
                        "var(--color-signal-500, #06b6d4)"
                    } else {
                        "var(--color-gray-300, #d1d5db)"
                    };
                    let detail_class = if n.warn { "fill-alarm" } else { "fill-slate" };
                    let click = n.id.clone();
                    let key = n.id.clone();
                    rsx! {
                        g {
                            key: "{n.id}",
                            class: "cursor-pointer",
                            role: "button",
                            tabindex: "0",
                            "aria-label": "{n.id}",
                            "data-testid": "table-node-{n.id}",
                            onclick: move |_| on_select.call(click.clone()),
                            onkeydown: move |e: KeyboardEvent| {
                                if e.key() == Key::Enter {
                                    on_select.call(key.clone());
                                }
                            },
                            rect {
                                x: "{x}",
                                y: "{y}",
                                width: "{NODE_W}",
                                height: "{NODE_H}",
                                rx: "8",
                                class: "fill-surface",
                                stroke,
                                stroke_width: if active { "2" } else { "1" },
                            }
                            text {
                                x: "{x + 12.0}",
                                y: "{y + 22.0}",
                                font_size: "13",
                                font_weight: "600",
                                class: "fill-ink",
                                "{short(&n.id)}"
                                title { "{n.id}" }
                            }
                            text {
                                x: "{x + 12.0}",
                                y: "{y + 41.0}",
                                font_size: "11",
                                class: "{detail_class}",
                                "{n.detail}"
                            }
                        }
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pnex_core::pipelines::Transform;

    fn node(id: &str) -> GraphNode {
        GraphNode {
            id: id.into(),
            detail: String::new(),
            warn: false,
        }
    }

    fn edge(a: &str, b: &str) -> (String, String) {
        (a.into(), b.into())
    }

    #[test]
    fn a_node_sits_after_its_longest_input() {
        let nodes = [node("a"), node("b"), node("c"), node("d")];
        // `d` reads `a` directly and through `b` and `c`.
        let edges = [
            edge("a", "b"),
            edge("b", "c"),
            edge("c", "d"),
            edge("a", "d"),
        ];
        let col = columns(&nodes, &edges);
        assert_eq!(col["a"], 0);
        assert_eq!(col["c"], 2);
        assert_eq!(col["d"], 3);
    }

    #[test]
    fn nodes_of_a_column_do_not_overlap_and_the_frame_holds_them() {
        let nodes = [node("a"), node("b"), node("c")];
        let edges = [edge("a", "c"), edge("b", "c")];
        let (pos, (w, h)) = layout(&nodes, &edges);
        assert_eq!(pos["a"].0, pos["b"].0);
        assert!(pos["b"].1 >= pos["a"].1 + NODE_H);
        assert!(pos["c"].0 + NODE_W <= w);
        assert!(pos["b"].1 + NODE_H <= h);
    }

    #[test]
    fn a_cycle_does_not_hang_the_layout() {
        let nodes = [node("a"), node("b")];
        let col = columns(&nodes, &[edge("a", "b"), edge("b", "a")]);
        assert!(col.values().all(|c| *c < 2));
    }

    #[test]
    fn a_document_gives_each_table_once() {
        let t = |output: &str, reads: &[&str]| Transform {
            output: output.into(),
            inputs: reads
                .iter()
                .enumerate()
                .map(|(i, r)| (format!("t{i}"), r.to_string()))
                .collect(),
            sql: "SELECT 1".into(),
            checks: Default::default(),
        };
        let doc = PipelineDocument {
            transforms: vec![t("clean", &["raw", "ref"]), t("final", &["clean", "ref"])],
        };
        let (names, edges) = document_graph(&doc);
        assert_eq!(names, ["raw", "ref", "clean", "final"]);
        assert_eq!(edges.len(), 4);
    }
}
