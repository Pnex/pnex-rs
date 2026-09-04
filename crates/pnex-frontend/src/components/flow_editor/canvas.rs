//! Rendu SVG + gestes du canevas de l'éditeur de flows.
//!
//! Palette (ajout de nœuds) et Canvas (SVG sans `view_box` : unité
//! utilisateur = 1 px CSS, cf. `geometry.rs`). Les handlers `move`/`up`
//! vivent sur le root SVG — fiable quand le pointeur quitte le nœud — et un
//! `pointerleave` annule tout geste dont le `pointerup` se serait perdu.

use dioxus::prelude::*;
use dioxus_i18n::t;
use pnex_core::{FlowNode, Position};

use super::geometry;
use super::state::{self, PaletteKind};
use super::{EditorCx, Interaction};

/// Palette gauche : un bouton par kind — clic = ajout au centre du viewport
/// (cascade via `geometry::cascade_origin`).
#[component]
pub(crate) fn Palette(cx: EditorCx) -> Element {
    let kinds = [
        PaletteKind::Inject,
        PaletteKind::PnexSql,
        PaletteKind::Device,
        PaletteKind::Calc,
        PaletteKind::Metric,
        PaletteKind::Debug,
        PaletteKind::Red,
    ];
    let entries: Vec<(PaletteKind, String, String)> = kinds
        .iter()
        .map(|kind| {
            let (label, help) = kind_labels(*kind);
            (*kind, label, help)
        })
        .collect();

    rsx! {
        div { class: "w-48 shrink-0 bg-white rounded-lg shadow-sm p-3 space-y-2 overflow-y-auto",
            h3 { class: "text-xs font-semibold text-gray-400 uppercase tracking-wider px-1", {t!("flows-palette-title")} }
            for (kind, label, help) in entries {
                button {
                    key: "{kind:?}",
                    class: "w-full text-left px-3 py-2 rounded-lg border border-gray-200 hover:border-blue-300 hover:bg-blue-50 transition-colors",
                    onclick: move |_| add_node_to_canvas(cx, kind),
                    div { class: "text-sm font-medium text-gray-800", {label} }
                    div { class: "text-xs text-gray-500", {help} }
                }
            }
        }
    }
}

/// Libellés i18n d'un kind (littéraux obligatoires pour `t!`).
pub(crate) fn kind_labels(kind: PaletteKind) -> (String, String) {
    match kind {
        PaletteKind::Inject => (t!("flows-palette-inject").to_string(), t!("flows-palette-inject-help").to_string()),
        PaletteKind::PnexSql => (t!("flows-palette-pnex-sql").to_string(), t!("flows-palette-pnex-sql-help").to_string()),
        PaletteKind::Device => (t!("flows-palette-device").to_string(), t!("flows-palette-device-help").to_string()),
        PaletteKind::Calc => (t!("flows-palette-calc").to_string(), t!("flows-palette-calc-help").to_string()),
        PaletteKind::Metric => (t!("flows-palette-metric").to_string(), t!("flows-palette-metric-help").to_string()),
        PaletteKind::Debug => (t!("flows-palette-debug").to_string(), t!("flows-palette-debug-help").to_string()),
        PaletteKind::Red => (t!("flows-palette-red").to_string(), t!("flows-palette-red-help").to_string()),
    }
}

/// Ajoute un nœud du kind au centre visible du canevas et le sélectionne.
fn add_node_to_canvas(mut cx: EditorCx, kind: PaletteKind) {
    let Some(rect) = geometry::canvas_rect() else {
        return;
    };
    let pan = cx.pan.cloned();
    let zoom = cx.zoom.cloned();
    let center = geometry::to_graph((rect.0 + rect.2 / 2.0, rect.1 + rect.3 / 2.0), rect, pan, zoom);
    let id = state::next_node_id(&cx.graph.peek().clone());
    let new_id = id.clone();
    cx.update_graph(move |graph| {
        let node = state::make_node(&new_id, kind, geometry::cascade_origin(graph, center));
        graph.nodes.push(node);
    });
    cx.selected_node.set(Some(id));
}

/// Segment de câble résolu : (source, port, cible, ancre départ, ancre
/// arrivée) — les ancres sont en coordonnées graphe.
type WireSeg = (String, usize, String, (f64, f64), (f64, f64));

/// Le canevas : grille, câbles (double path hit+visible), nœuds, câble
/// temporaire de câblage. SANS `view_box` — 1 unité = 1 px CSS.
#[component]
pub(crate) fn Canvas(mut cx: EditorCx, on_cut_wire: Callback<(String, usize, String)>) -> Element {
    let graph = cx.graph.cloned();
    let pan = cx.pan.cloned();
    let zoom = cx.zoom.cloned();
    let interaction = cx.interaction.cloned();

    // Câbles résolus : (source, port, cible, ancre départ, ancre arrivée).
    let mut wires: Vec<WireSeg> = Vec::new();
    for node in &graph.nodes {
        let Some(pos) = node.position else { continue };
        for wiring in &node.outputs {
            for target_id in &wiring.targets {
                let Some(target) = graph.nodes.iter().find(|n| &n.id == target_id) else {
                    continue;
                };
                let Some(target_pos) = target.position else { continue };
                wires.push((
                    node.id.clone(),
                    wiring.port,
                    target_id.clone(),
                    geometry::port_out(pos),
                    geometry::port_in(target_pos),
                ));
            }
        }
    }

    rsx! {
        div { class: "relative flex-1 bg-white rounded-lg shadow-sm overflow-hidden",
            svg {
                id: "flow-canvas",
                class: "absolute inset-0 w-full h-full",
                tabindex: "0",
                onpointerdown: move |event| canvas_pointer_down(event, cx),
                onpointermove: move |event| canvas_pointer_move(event, cx),
                onpointerup: move |_event| canvas_pointer_up(cx),
                onpointerleave: move |_| {
                    // Geste orphelin (pointup hors canvas) : annulation.
                    cx.interaction.set(Interaction::Idle);
                },
                onwheel: move |event| canvas_wheel(event, cx),
                onkeydown: move |event| {
                    let is_delete =
                        event.key() == Key::Delete || event.key() == Key::Backspace;
                    if !is_delete {
                        return;
                    }
                    if let Some(id) = cx.selected_node.cloned() {
                        cx.update_graph(|graph| state::remove_node(graph, &id));
                        cx.selected_node.set(None);
                    }
                },
                defs {
                    pattern {
                        id: "flow-grid",
                        width: "20",
                        height: "20",
                        "patternUnits": "userSpaceOnUse",
                        path {
                            d: "M 20 0 L 0 0 0 20",
                            fill: "none",
                            stroke: "#f3f4f6",
                            "stroke-width": "1",
                        }
                    }
                }
                rect { x: "0", y: "0", width: "100%", height: "100%", fill: "url(#flow-grid)" }
                g {
                    transform: "translate({pan.0} {pan.1}) scale({zoom})",
                    // Câbles — d'abord la couche de hit (transparente, large),
                    // puis la couche visible.
                    for (source_id, port, target_id, a, b) in wires.clone() {
                        path {
                            key: "hit-{source_id}-{port}-{target_id}",
                            d: geometry::wire_path(a, b),
                            fill: "none",
                            stroke: "transparent",
                            "stroke-width": "12",
                            "pointer-events": "stroke",
                            onpointerdown: move |event| {
                                event.stop_propagation();
                                on_cut_wire.call((source_id.clone(), port, target_id.clone()));
                            },
                        }
                    }
                    for (source_id, port, target_id, a, b) in wires {
                        path {
                            key: "wire-{source_id}-{port}-{target_id}",
                            d: geometry::wire_path(a, b),
                            fill: "none",
                            stroke: geometry::WIRE_STROKE,
                            "stroke-width": "2",
                            "pointer-events": "none",
                        }
                    }
                    if let Interaction::Wiring { from_id, cursor, .. } = interaction {
                        // Câble temporaire pendant le câblage.
                        path {
                            d: geometry::wire_path(temp_origin(&graph, &from_id), cursor),
                            fill: "none",
                            stroke: geometry::SELECTED_STROKE,
                            "stroke-width": "2",
                            "stroke-dasharray": "6 4",
                            "pointer-events": "none",
                        }
                    }
                    for node in &graph.nodes {
                        CanvasNode {
                            key: "{node.id}",
                            node: node.clone(),
                            cx,
                        }
                    }
                }
            }
            if graph.nodes.is_empty() {
                div { class: "absolute inset-0 flex items-center justify-center pointer-events-none",
                    p { class: "text-sm text-gray-400", {t!("flows-canvas-empty-hint")} }
                }
            }
        }
    }
}

/// Ancre de départ du câble temporaire (0,0 si la source a disparu — cas
/// défensif, le hit-test exclut déjà la source).
fn temp_origin(graph: &pnex_core::FlowGraph, id: &str) -> (f64, f64) {
    graph
        .nodes
        .iter()
        .find(|n| n.id == id)
        .and_then(|n| n.position)
        .map(geometry::port_out)
        .unwrap_or((0.0, 0.0))
}

// ─────────────────────────────── Gestes ───────────────────────────────

/// Client (px écran) → tuple.
fn client_of(event: &PointerEvent) -> (f64, f64) {
    let point = event.client_coordinates();
    (point.x, point.y)
}

/// pointerdown fond : désélection + début du pan.
fn canvas_pointer_down(event: PointerEvent, mut cx: EditorCx) {
    cx.selected_node.set(None);
    cx.interaction.set(Interaction::Panning {
        start_client: client_of(&event),
        start_pan: cx.pan.cloned(),
    });
}

/// pointermove : agit selon le geste en cours (le handler vit sur le root —
/// le pointeur peut sortir du nœud sans casser le drag).
fn canvas_pointer_move(event: PointerEvent, mut cx: EditorCx) {
    let client = client_of(&event);
    match cx.interaction.cloned() {
        Interaction::Idle => {}
        Interaction::Panning { start_client, start_pan } => {
            cx.pan.set((
                start_pan.0 + client.0 - start_client.0,
                start_pan.1 + client.1 - start_client.1,
            ));
        }
        Interaction::Dragging { id, grab, rect } => {
            let point = geometry::to_graph(client, rect, cx.pan.cloned(), cx.zoom.cloned());
            let pos = Position {
                x: geometry::snap(point.0 - grab.0),
                y: geometry::snap(point.1 - grab.1),
            };
            cx.update_graph(move |graph| state::move_node(graph, &id, pos));
        }
        Interaction::Wiring { from_id, port, .. } => {
            let Some(rect) = geometry::canvas_rect() else {
                return;
            };
            let cursor = geometry::to_graph(client, rect, cx.pan.cloned(), cx.zoom.cloned());
            let hover_target = geometry::node_at(&cx.graph.peek(), cursor)
                .map(|node| node.id.clone())
                .filter(|id| id != &from_id);
            cx.interaction.set(Interaction::Wiring {
                from_id,
                port,
                cursor,
                hover_target,
            });
        }
    }
}

/// pointerup : finalise le câblage éventuel, sinon retour à l'idle.
fn canvas_pointer_up(mut cx: EditorCx) {
    match cx.interaction.cloned() {
        Interaction::Wiring { from_id, port, hover_target: Some(target), .. } => {
            cx.update_graph(move |graph| state::add_target(graph, &from_id, port, &target));
            cx.interaction.set(Interaction::Idle);
        }
        _ => cx.interaction.set(Interaction::Idle),
    }
}

/// Molette : zoom vers le curseur, borné (`ZOOM_MIN/MAX`).
fn canvas_wheel(event: WheelEvent, mut cx: EditorCx) {
    event.prevent_default();
    let delta_y = match event.delta() {
        dioxus::html::geometry::WheelDelta::Pixels(point) => point.y,
        dioxus::html::geometry::WheelDelta::Lines(point) => point.y * 16.0,
        dioxus::html::geometry::WheelDelta::Pages(point) => point.y * 100.0,
    };
    let Some(rect) = geometry::canvas_rect() else {
        return;
    };
    let point = event.client_coordinates();
    let client = (point.x, point.y);
    let old = cx.zoom.cloned();
    // Un cran ≈ 10 % — multiplicatif pour une sensation constante.
    let factor = if delta_y < 0.0 { 1.1 } else { 1.0 / 1.1 };
    let new = (old * factor).clamp(geometry::ZOOM_MIN, geometry::ZOOM_MAX);
    if (new - old).abs() < f64::EPSILON {
        return;
    }
    let screen = (client.0 - rect.0, client.1 - rect.1);
    cx.pan.set(geometry::zoom_pan_towards(cx.pan.cloned(), old, new, screen));
    cx.zoom.set(new);
}

// ─────────────────────────────── Nœud ───────────────────────────────

/// Un nœud du graphe : rectangle coloré, libellé, ports. Le `pointerdown`
/// sélectionne et amorce le drag ; le port de sortie amorce le câblage.
#[component]
fn CanvasNode(mut cx: EditorCx, node: FlowNode) -> Element {
    let Some(pos) = node.position else {
        return rsx! {};
    };
    let selected = cx.selected_node.cloned().as_deref() == Some(node.id.as_str());
    let has_violation = cx
        .violations
        .read()
        .iter()
        .any(|v| v.node_id.as_deref() == Some(node.id.as_str()));

    let (kind_label, _) = match &node.kind {
        pnex_core::FlowNodeKind::Inject { .. } => {
            kind_labels(PaletteKind::Inject)
        }
        pnex_core::FlowNodeKind::PnexSql { .. } => kind_labels(PaletteKind::PnexSql),
        pnex_core::FlowNodeKind::Device { .. } => kind_labels(PaletteKind::Device),
        pnex_core::FlowNodeKind::Calc { .. } => kind_labels(PaletteKind::Calc),
        pnex_core::FlowNodeKind::Metric { .. } => kind_labels(PaletteKind::Metric),
        pnex_core::FlowNodeKind::Debug { .. } => kind_labels(PaletteKind::Debug),
        pnex_core::FlowNodeKind::Red { .. } => kind_labels(PaletteKind::Red),
    };

    let (fill, stroke) = match &node.kind {
        pnex_core::FlowNodeKind::Inject { .. } => (geometry::INJECT_FILL, geometry::INJECT_STROKE),
        pnex_core::FlowNodeKind::PnexSql { .. } => (geometry::SQL_FILL, geometry::SQL_STROKE),
        pnex_core::FlowNodeKind::Device { .. } => (geometry::DEVICE_FILL, geometry::DEVICE_STROKE),
        pnex_core::FlowNodeKind::Calc { .. } => (geometry::CALC_FILL, geometry::CALC_STROKE),
        pnex_core::FlowNodeKind::Metric { .. } => (geometry::METRIC_FILL, geometry::METRIC_STROKE),
        pnex_core::FlowNodeKind::Debug { .. } => (geometry::DEBUG_FILL, geometry::DEBUG_STROKE),
        pnex_core::FlowNodeKind::Red { .. } => (geometry::RED_FILL, geometry::RED_STROKE),
    };
    let stroke = if has_violation {
        geometry::VIOLATION_STROKE
    } else if selected {
        geometry::SELECTED_STROKE
    } else {
        stroke
    };

    // Libellé affiché : nom du nœud, sinon libellé du kind.
    let title = node.name.clone().unwrap_or_else(|| kind_label.clone());
    // Sous-titre : résumé de config (donnée brute, pas d'i18n).
    let subtitle = node_subtitle(&node);
    // Capturé par chaque closure (le composant capture `node` une seule fois).
    let node_id = node.id.clone();
    let node_id_port = node.id.clone();

    rsx! {
        g {
            transform: "translate({pos.x} {pos.y})",
            onpointerdown: move |event| {
                event.stop_propagation();
                let Some(rect) = geometry::canvas_rect() else { return; };
                let client = client_of(&event);
                let point = geometry::to_graph(client, rect, cx.pan.cloned(), cx.zoom.cloned());
                let grab = (point.0 - pos.x, point.1 - pos.y);
                cx.selected_node.set(Some(node_id.clone()));
                cx.interaction.set(Interaction::Dragging { id: node_id.clone(), grab, rect });
            },
            rect {
                width: "{geometry::NODE_W}",
                height: "{geometry::NODE_H}",
                rx: "8",
                fill: "{fill}",
                stroke: "{stroke}",
                "stroke-width": "2",
                style: "cursor: grab",
            }
            text {
                x: "{geometry::NODE_W / 2.0}",
                y: "20",
                "text-anchor": "middle",
                "font-size": "12",
                "font-weight": "600",
                fill: "#1f2937",
                "pointer-events": "none",
                {title}
            }
            text {
                x: "{geometry::NODE_W / 2.0}",
                y: "36",
                "text-anchor": "middle",
                "font-size": "10",
                fill: "#6b7280",
                "pointer-events": "none",
                {subtitle}
            }
            // Port d'entrée (visuel).
            circle {
                cx: "0",
                cy: "{geometry::NODE_H / 2.0}",
                r: "4",
                fill: "{stroke}",
                "pointer-events": "none",
            }
            // Port de sortie (amorce le câblage).
            circle {
                cx: "{geometry::NODE_W}",
                cy: "{geometry::NODE_H / 2.0}",
                r: "7",
                fill: "{stroke}",
                style: "cursor: crosshair",
                onpointerdown: move |event| {
                    event.stop_propagation();
                    let Some(rect) = geometry::canvas_rect() else { return; };
                    let client = client_of(&event);
                    let cursor = geometry::to_graph(client, rect, cx.pan.cloned(), cx.zoom.cloned());
                    cx.selected_node.set(Some(node_id_port.clone()));
                    cx.interaction.set(Interaction::Wiring {
                        from_id: node_id_port.clone(),
                        port: 0,
                        cursor,
                        hover_target: None,
                    });
                },
            }
        }
    }
}

/// Résumé de config affiché sous le libellé (donnée, pas d'i18n).
fn node_subtitle(node: &FlowNode) -> String {
    match &node.kind {
        pnex_core::FlowNodeKind::Inject { config } => {
            if let Some(repeat) = config.repeat_secs {
                format!("{repeat} s")
            } else if !config.cron.is_empty() {
                config.cron.clone()
            } else if let Some(delay) = config.once_delay_secs {
                format!("+{delay} s")
            } else {
                "—".into()
            }
        }
        pnex_core::FlowNodeKind::PnexSql { config } => {
            let first = config.query.lines().next().unwrap_or_default();
            // Troncation sûre (caractères, pas d'octets — SQL accentué).
            let short: String = first.chars().take(22).collect();
            if first.chars().count() > 22 {
                format!("{short}…")
            } else {
                short
            }
        }
        pnex_core::FlowNodeKind::Device { config } => {
            let n = config.reads.len();
            if n == 0 {
                "—".into()
            } else {
                format!("{n} lecture(s) · {} s", config.window_secs)
            }
        }
        pnex_core::FlowNodeKind::Calc { config } => {
            let short: String = config.expression.chars().take(22).collect();
            if config.expression.chars().count() > 22 {
                format!("{short}…")
            } else if short.is_empty() {
                "—".into()
            } else {
                short
            }
        }
        pnex_core::FlowNodeKind::Metric { config } => {
            if config.metric_name.is_empty() {
                "—".into()
            } else {
                pnex_core::etl_metric_name(&config.metric_name)
            }
        }
        pnex_core::FlowNodeKind::Debug { .. } => "debug".into(),
        pnex_core::FlowNodeKind::Red { type_name, .. } => {
            if type_name.is_empty() {
                "—".into()
            } else {
                type_name.clone()
            }
        }
    }
}
