//! Rendu SVG + gestes du canevas de l'éditeur de flows.
//!
//! Palette (ajout de nœuds) et Canvas (SVG sans `view_box` : unité
//! utilisateur = 1 px CSS, cf. `geometry.rs`). Les handlers `move`/`up`
//! vivent sur le root SVG — fiable quand le pointeur quitte le nœud — et un
//! `pointerleave` annule tout geste dont le `pointerup` se serait perdu.

use dioxus::prelude::*;
use dioxus_i18n::t;
use pnex_core::{FlowNode, FlowNodeKind, Position};

use super::debug;
use super::geometry;
use super::state::{self, PaletteKind};
use super::{EditorCx, Interaction};
use crate::components::editor_shell::{PaletteIcon, PaletteItem};

mod gestures;
mod palette;
mod pinch;

use gestures::*;
pub(crate) use palette::*;
/// Resolved wire segment: (source, source port, target, start anchor, end
/// anchor, targeted input row) — anchors are in graph coordinates; the row
/// label is non-empty for input-annotated wires (device-write pin rows,
/// function input rows), which the cut callback needs to remove the right
/// annotation.
type WireSeg = (String, usize, String, (f64, f64), (f64, f64), String);

/// Le canevas : grille, câbles (double path hit+visible), nœuds, câble
/// temporaire de câblage. SANS `view_box` — 1 unité = 1 px CSS.
#[component]
pub(crate) fn Canvas(
    mut cx: EditorCx,
    on_cut_wire: Callback<(String, usize, String, String)>,
) -> Element {
    let graph = cx.graph.cloned();
    let pan = cx.pan.cloned();
    let zoom = cx.zoom.cloned();
    let interaction = cx.interaction.cloned();
    pinch::use_pinch_zoom(cx);
    crate::components::dom_rect::use_rect_tracker("flow-canvas");

    // Frame every node once the graph is loaded (O51): a saved layout can
    // start outside the visible area.
    let mut fitted = use_signal(|| false);
    use_effect(move || {
        let has_nodes = !cx.graph.read().nodes.is_empty();
        if has_nodes && !*fitted.peek() && fit_to_nodes(cx) {
            fitted.set(true);
        }
    });

    // Nœuds en violation (validation sauvegarde + staleness pin/device) —
    // un câble est rouge dès qu'une de ses extrémités l'est.
    let violation_nodes: std::collections::HashSet<String> = cx
        .violations
        .read()
        .iter()
        .filter_map(|v| v.node_id.clone())
        .chain(cx.stale.read().iter().filter_map(|v| v.node_id.clone()))
        .collect();

    // Resolved wires: the start anchor follows the source's multi-port
    // layout (device-read). A wire into a device-write is drawn from the
    // target's input annotation when one exists (one per targeted pin row —
    // the row is chosen at drop); a wire without annotation (drawn before
    // its target row was chosen) uses the nearest row.
    let mut wires: Vec<WireSeg> = Vec::new();
    for node in &graph.nodes {
        let Some(pos) = node.position else { continue };
        let out_count = state::output_count_of(node);
        let src_h = geometry::node_height_for(node);
        for wiring in &node.outputs {
            for target_id in &wiring.targets {
                let Some(target) = graph.nodes.iter().find(|n| &n.id == target_id) else {
                    continue;
                };
                let annotated = target.inputs.iter().any(|w| {
                    w.from == node.id
                        && w.from_port == wiring.port
                        && geometry::input_row_labels(target)
                            .iter()
                            .any(|l| l == &w.pin)
                });
                if annotated {
                    continue; // drawn from the target's inputs below
                }
                let Some(target_pos) = target.position else {
                    continue;
                };
                let (sx, sy) = geometry::port_out_at(pos, wiring.port, out_count, src_h);
                wires.push((
                    node.id.clone(),
                    wiring.port,
                    target_id.clone(),
                    (sx, sy),
                    geometry::plain_wire_landing(
                        target_pos,
                        target,
                        geometry::node_height_for(target),
                        sy,
                    ),
                    String::new(),
                ));
            }
        }
    }
    // Input-annotated wires: one per targeted row (device-write pins,
    // function inputs). Stale annotations (row gone since) are skipped.
    for node in &graph.nodes {
        if node.inputs.is_empty() {
            continue;
        }
        let Some(pos) = node.position else { continue };
        let target_h = geometry::node_height_for(node);
        let labels = geometry::input_row_labels(node);
        let rows = geometry::input_anchor_rows(node, target_h);
        for annotation in &node.inputs {
            let Some(source) = graph.nodes.iter().find(|n| n.id == annotation.from) else {
                continue;
            };
            let Some(src_pos) = source.position else {
                continue;
            };
            let Some(row_index) = labels.iter().position(|p| p == &annotation.pin) else {
                continue; // stale annotation (row gone since)
            };
            let Some(&row_y) = rows.get(row_index) else {
                continue;
            };
            let (sx, sy) = geometry::port_out_at(
                src_pos,
                annotation.from_port,
                state::output_count_of(source),
                geometry::node_height_for(source),
            );
            wires.push((
                annotation.from.clone(),
                annotation.from_port,
                node.id.clone(),
                (sx, sy),
                (pos.x, pos.y + row_y),
                annotation.pin.clone(),
            ));
        }
    }

    // Data type of every wire (colour + midpoint label, O70).
    let wire_kinds: Vec<super::wire_types::DataType> = wires
        .iter()
        .map(|(source_id, port, ..)| super::wire_types::output_type(&graph, source_id, *port))
        .collect();

    // Câble temporaire pendant le câblage — origine = ancre de sortie
    // (câblage direct) ou d'entrée (inverse). Pré-calcul hors rsx (pas de
    // `let` dans le rsx).
    let temp_wire: Option<((f64, f64), (f64, f64))> = match &interaction {
        Interaction::Wiring {
            from_id,
            port,
            cursor,
            reverse,
            pin,
            ..
        } => Some((
            if *reverse {
                temp_input_origin(&graph, from_id, pin.as_deref())
            } else {
                temp_origin(&graph, from_id, *port)
            },
            *cursor,
        )),
        _ => None,
    };

    rsx! {
        div { class: "relative h-full w-full bg-white overflow-hidden select-none",
            svg {
                id: "flow-canvas",
                // `touch-none`: the browser must not claim touches for its
                // own scroll / zoom — it fired `pointercancel` after a few
                // millimetres and a one-finger pan or drag stopped dead.
                class: "absolute inset-0 w-full h-full touch-none outline-none",
                tabindex: "0",
                // No native text selection on the canvas: a node drag must
                // never highlight the labels it sweeps over (all gestures
                // are handled by the pointer events below — the browser
                // default only ever selects text here).
                onmousedown: move |event: MouseEvent| event.prevent_default(),
                onpointerdown: move |event| canvas_pointer_down(event, cx),
                onpointermove: move |event| canvas_pointer_move(event, cx),
                onpointerup: move |_event| canvas_pointer_up(cx),
                onpointerleave: move |_| {
                    // Geste orphelin (pointup hors canvas) : annulation.
                    end_gesture(cx);
                },
                onwheel: move |event| canvas_wheel(event, cx),
                onkeydown: move |event| {
                    // Échap : désélection (l'inspecteur se replie avec).
                    if event.key() == Key::Escape {
                        cx.selected_node.set(None);
                        return;
                    }
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
                            stroke: "var(--color-gray-100, #f3f4f6)",
                            "stroke-width": "1",
                        }
                    }
                }
                // Câbles — d'abord la couche de hit (transparente, large),
                // puis la couche visible.
                rect {
                    x: "0",
                    y: "0",
                    width: "100%",
                    height: "100%",
                    fill: "url(#flow-grid)",
                }
                g { transform: "translate({pan.0} {pan.1}) scale({zoom})",
                    for (wire_index, (source_id, port, target_id, a, b, pin)) in wires.clone().into_iter().enumerate() {
                        path {
                            // Index dans la clé : des annotations dupliquées
                            // (renommage en doublon, graphe transitoirement
                            // invalide — la validation le signale) ne doivent
                            // jamais paniquer le rendu (« keyed siblings »).
                            key: "hit-{wire_index}-{source_id}-{port}-{target_id}-{pin}",
                            d: geometry::wire_path(a, b),
                            fill: "none",
                            stroke: "transparent",
                            "stroke-width": "12",
                            "pointer-events": "stroke",
                            onpointerdown: move |event| {
                                event.stop_propagation();
                                on_cut_wire.call((source_id.clone(), port, target_id.clone(), pin.clone()));
                            },
                        }
                    }
                    for (wire_index, ((source_id, port, target_id, a, b, pin), kind)) in wires.into_iter().zip(wire_kinds).enumerate() {
                        path {
                            key: "wire-{wire_index}-{source_id}-{port}-{target_id}-{pin}",
                            d: geometry::wire_path(a, b),
                            fill: "none",
                            // Câble rouge quand une extrémité porte une
                            // violation (graphe invalide ou staleness
                            // pin/device — le lien cassé saute aux yeux).
                            stroke: if violation_nodes.contains(&source_id) || violation_nodes.contains(&target_id) { geometry::VIOLATION_STROKE } else { kind.color() },
                            "stroke-width": "2",
                            "pointer-events": "none",
                        }
                        // Type label at the wire midpoint (the curve is
                        // symmetric: t = 0.5 is the chord midpoint).
                        if kind != super::wire_types::DataType::Any {
                            text {
                                key: "wire-type-{wire_index}-{source_id}-{port}-{target_id}-{pin}",
                                x: "{(a.0 + b.0) / 2.0}",
                                // Centred on the wire, the halo masks it under
                                // the text (half above, it crossed the glyphs).
                                y: "{(a.1 + b.1) / 2.0 + 3.0}",
                                "text-anchor": "middle",
                                "font-size": "9",
                                fill: kind.color(),
                                stroke: "var(--color-surface, #ffffff)",
                                "stroke-width": "6",
                                "stroke-linejoin": "round",
                                "paint-order": "stroke",
                                "pointer-events": "none",
                                {kind.label()}
                            }
                        }
                    }
                    // Câble temporaire pendant le câblage — né sur l'ancre
                    // de sortie (câblage direct) ou d'entrée (inverse).
                    // Pré-calcul hors rsx (pas de `let` dans le rsx).
                    if let Some((origin, cursor)) = temp_wire {
                        path {
                            d: geometry::wire_path(origin, cursor),
                            fill: "none",
                            stroke: geometry::SELECTED_STROKE,
                            "stroke-width": "2",
                            "stroke-dasharray": "6 4",
                            "pointer-events": "none",
                        }
                    }
                    for node in &graph.nodes {
                        CanvasNode { key: "{node.id}", node: node.clone(), cx }
                    }
                }
            }
            button {
                class: "{crate::components::ui::BTN_SECONDARY_SM} absolute bottom-3 left-3 z-10",
                r#type: "button",
                title: t!("flows-fit-view-help"),
                onclick: move |_| {
                    fit_to_nodes(cx);
                },
                crate::components::icons::Maximize { class: "h-4 w-4" }
                {t!("flows-fit-view")}
            }
        }
    }
}

/// Pans and zooms the canvas onto every node; `false` when there is nothing
/// to frame or the canvas is not measured yet.
fn fit_to_nodes(mut cx: EditorCx) -> bool {
    let Some(rect) = geometry::canvas_rect() else {
        return false;
    };
    let Some((pan, zoom)) = geometry::fit_view(&cx.graph.peek().nodes, rect.2, rect.3) else {
        return false;
    };
    cx.pan.set(pan);
    cx.zoom.set(zoom);
    true
}

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
        .chain(cx.stale.read().iter())
        .any(|v| v.node_id.as_deref() == Some(node.id.as_str()));

    let (kind_label, _) = match &node.kind {
        pnex_core::FlowNodeKind::Inject { .. } => kind_labels(PaletteKind::Inject),
        pnex_core::FlowNodeKind::Value { .. } => kind_labels(PaletteKind::Value),
        pnex_core::FlowNodeKind::DeviceRead { .. } => kind_labels(PaletteKind::DeviceRead),
        pnex_core::FlowNodeKind::DeviceWrite { .. } => kind_labels(PaletteKind::DeviceWrite),
        pnex_core::FlowNodeKind::Calc { .. } => kind_labels(PaletteKind::Calc),
        pnex_core::FlowNodeKind::Metric { .. } => kind_labels(PaletteKind::Metric),
        pnex_core::FlowNodeKind::CoolProp { .. } => kind_labels(PaletteKind::CoolProp),
        pnex_core::FlowNodeKind::Display { .. } => kind_labels(PaletteKind::Display),
        pnex_core::FlowNodeKind::RegTtHeat { .. } => kind_labels(PaletteKind::RegTtHeat),
        pnex_core::FlowNodeKind::RegTtCool { .. } => kind_labels(PaletteKind::RegTtCool),
        pnex_core::FlowNodeKind::RegPid { .. } => kind_labels(PaletteKind::RegPid),
        pnex_core::FlowNodeKind::PnexNotify { .. } => kind_labels(PaletteKind::PnexNotify),
        pnex_core::FlowNodeKind::HttpFetch { .. } => kind_labels(PaletteKind::HttpFetch),
        pnex_core::FlowNodeKind::PnexFunction { .. } => kind_labels(PaletteKind::Function),
        pnex_core::FlowNodeKind::JsonSplit { .. } => kind_labels(PaletteKind::JsonSplit),
        pnex_core::FlowNodeKind::JsonMerge { .. } => kind_labels(PaletteKind::JsonMerge),
        pnex_core::FlowNodeKind::CameraSource { .. } => kind_labels(PaletteKind::CameraSource),
        pnex_core::FlowNodeKind::MediaSource { .. } => kind_labels(PaletteKind::MediaSource),
        pnex_core::FlowNodeKind::TopicClassify { .. } => kind_labels(PaletteKind::TopicClassify),
        pnex_core::FlowNodeKind::RangeUpsert { .. } => kind_labels(PaletteKind::RangeUpsert),
        pnex_core::FlowNodeKind::VideoRecord { .. } => kind_labels(PaletteKind::VideoRecord),
        pnex_core::FlowNodeKind::VisionDetect { .. } => kind_labels(PaletteKind::VisionDetect),
        pnex_core::FlowNodeKind::VisionTrack { .. } => kind_labels(PaletteKind::VisionTrack),
        pnex_core::FlowNodeKind::VisionZone { .. } => kind_labels(PaletteKind::VisionZone),
        pnex_core::FlowNodeKind::VisionCaption { .. } => kind_labels(PaletteKind::VisionCaption),
        pnex_core::FlowNodeKind::MediaFuse { .. } => kind_labels(PaletteKind::MediaFuse),
        pnex_core::FlowNodeKind::LlmText { .. } => kind_labels(PaletteKind::LlmText),
        pnex_core::FlowNodeKind::PageWrite { .. } => kind_labels(PaletteKind::PageWrite),
        pnex_core::FlowNodeKind::PagePdf { .. } => kind_labels(PaletteKind::PagePdf),
        pnex_core::FlowNodeKind::TextDecide { .. } => kind_labels(PaletteKind::TextDecide),
        pnex_core::FlowNodeKind::EventLog { .. } => kind_labels(PaletteKind::EventLog),
        pnex_core::FlowNodeKind::MemoryWrite { .. } => kind_labels(PaletteKind::MemoryWrite),
        pnex_core::FlowNodeKind::MemoryRead { .. } => kind_labels(PaletteKind::MemoryRead),
        pnex_core::FlowNodeKind::ControlSource { .. } => kind_labels(PaletteKind::ControlSource),
        pnex_core::FlowNodeKind::ActionTrigger { .. } => kind_labels(PaletteKind::ActionTrigger),
        pnex_core::FlowNodeKind::OntologyQuery { .. } => kind_labels(PaletteKind::OntologyQuery),
        pnex_core::FlowNodeKind::ObjectUpsert { .. } => kind_labels(PaletteKind::ObjectUpsert),
        pnex_core::FlowNodeKind::LinkUpsert { .. } => kind_labels(PaletteKind::LinkUpsert),
        pnex_core::FlowNodeKind::Ingest { .. } => kind_labels(kind_of(&node.kind)),
        pnex_core::FlowNodeKind::SiteRecipe { .. } => kind_labels(PaletteKind::SiteRecipe),
        pnex_core::FlowNodeKind::Weather { .. } => kind_labels(PaletteKind::Weather),
        pnex_core::FlowNodeKind::Anomaly { .. } => kind_labels(PaletteKind::Anomaly),
        pnex_core::FlowNodeKind::Forecast { .. } => kind_labels(PaletteKind::Forecast),
        pnex_core::FlowNodeKind::Debug { .. } => kind_labels(PaletteKind::Debug),
    };

    let (fill, stroke) = match &node.kind {
        pnex_core::FlowNodeKind::Inject { .. } => (geometry::INJECT_FILL, geometry::INJECT_STROKE),
        pnex_core::FlowNodeKind::Value { .. } => (geometry::VALUE_FILL, geometry::VALUE_STROKE),
        pnex_core::FlowNodeKind::DeviceRead { .. } => {
            (geometry::DEVICE_FILL, geometry::DEVICE_STROKE)
        }
        pnex_core::FlowNodeKind::DeviceWrite { .. } => {
            (geometry::REG_TT_FILL, geometry::REG_TT_STROKE)
        }
        pnex_core::FlowNodeKind::Calc { .. } => (geometry::CALC_FILL, geometry::CALC_STROKE),
        pnex_core::FlowNodeKind::Metric { .. } => (geometry::METRIC_FILL, geometry::METRIC_STROKE),
        pnex_core::FlowNodeKind::CoolProp { .. } => {
            (geometry::COOLPROP_FILL, geometry::COOLPROP_STROKE)
        }
        pnex_core::FlowNodeKind::Display { .. } => {
            (geometry::DISPLAY_FILL, geometry::DISPLAY_STROKE)
        }
        pnex_core::FlowNodeKind::RegTtHeat { .. } => {
            (geometry::REG_TT_FILL, geometry::REG_TT_STROKE)
        }
        pnex_core::FlowNodeKind::RegTtCool { .. } => {
            (geometry::REG_TT_FILL, geometry::REG_TT_STROKE)
        }
        pnex_core::FlowNodeKind::RegPid { .. } => {
            (geometry::REG_PID_FILL, geometry::REG_PID_STROKE)
        }
        pnex_core::FlowNodeKind::PnexNotify { .. } => {
            (geometry::NOTIFY_FILL, geometry::NOTIFY_STROKE)
        }
        pnex_core::FlowNodeKind::HttpFetch { .. } => {
            (geometry::HTTP_FETCH_FILL, geometry::HTTP_FETCH_STROKE)
        }
        // Fonctions du registre : palette du calc (même famille « calcul »).
        pnex_core::FlowNodeKind::PnexFunction { .. } => {
            (geometry::CALC_FILL, geometry::CALC_STROKE)
        }
        // JSON split/merge : cyan/teal (famille transformation de données).
        pnex_core::FlowNodeKind::JsonSplit { .. } => {
            (geometry::JSON_SPLIT_FILL, geometry::JSON_SPLIT_STROKE)
        }
        pnex_core::FlowNodeKind::JsonMerge { .. }
        | pnex_core::FlowNodeKind::TopicClassify { .. } => {
            (geometry::JSON_MERGE_FILL, geometry::JSON_MERGE_STROKE)
        }
        pnex_core::FlowNodeKind::CameraSource { .. } => {
            (geometry::CAMERA_FILL, geometry::CAMERA_STROKE)
        }
        pnex_core::FlowNodeKind::VideoRecord { .. } => {
            (geometry::VIDEO_RECORD_FILL, geometry::VIDEO_RECORD_STROKE)
        }
        pnex_core::FlowNodeKind::VisionDetect { .. }
        | pnex_core::FlowNodeKind::VisionTrack { .. }
        | pnex_core::FlowNodeKind::VisionZone { .. }
        | pnex_core::FlowNodeKind::VisionCaption { .. }
        | pnex_core::FlowNodeKind::MediaFuse { .. }
        | pnex_core::FlowNodeKind::LlmText { .. }
        | pnex_core::FlowNodeKind::TextDecide { .. } => {
            (geometry::VISION_FILL, geometry::VISION_STROKE)
        }
        pnex_core::FlowNodeKind::EventLog { .. }
        | pnex_core::FlowNodeKind::RangeUpsert { .. }
        | pnex_core::FlowNodeKind::PageWrite { .. }
        | pnex_core::FlowNodeKind::PagePdf { .. } => {
            (geometry::EVENT_LOG_FILL, geometry::EVENT_LOG_STROKE)
        }
        pnex_core::FlowNodeKind::Anomaly { .. } => {
            (geometry::ANOMALY_FILL, geometry::ANOMALY_STROKE)
        }
        pnex_core::FlowNodeKind::Forecast { .. } => {
            (geometry::FORECAST_FILL, geometry::FORECAST_STROKE)
        }
        pnex_core::FlowNodeKind::MemoryWrite { .. } => {
            (geometry::MEMORY_WRITE_FILL, geometry::MEMORY_WRITE_STROKE)
        }
        pnex_core::FlowNodeKind::MemoryRead { .. }
        | pnex_core::FlowNodeKind::OntologyQuery { .. }
        | pnex_core::FlowNodeKind::ObjectUpsert { .. }
        | pnex_core::FlowNodeKind::LinkUpsert { .. }
        | pnex_core::FlowNodeKind::Ingest { .. }
        | pnex_core::FlowNodeKind::SiteRecipe { .. } => {
            (geometry::MEMORY_READ_FILL, geometry::MEMORY_READ_STROKE)
        }
        pnex_core::FlowNodeKind::ControlSource { .. }
        | pnex_core::FlowNodeKind::ActionTrigger { .. }
        | pnex_core::FlowNodeKind::Weather { .. }
        | pnex_core::FlowNodeKind::MediaSource { .. } => (
            geometry::CONTROL_SOURCE_FILL,
            geometry::CONTROL_SOURCE_STROKE,
        ),
        pnex_core::FlowNodeKind::Debug { .. } => (geometry::DEBUG_FILL, geometry::DEBUG_STROKE),
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
    // Badge live de la sonde : dernière valeur publiée (source pnex-display
    // uniquement), vidée au stop moteur / save / deploy par le parent.
    // Debug nodes too: their last message, visible in every mode.
    let display_badge = match &node.kind {
        pnex_core::FlowNodeKind::Display { .. }
        | pnex_core::FlowNodeKind::Debug { .. }
        | pnex_core::FlowNodeKind::CameraSource { .. }
        | pnex_core::FlowNodeKind::ControlSource { .. }
        | pnex_core::FlowNodeKind::VisionDetect { .. }
        | pnex_core::FlowNodeKind::VisionCaption { .. }
        | pnex_core::FlowNodeKind::MediaFuse { .. }
        | pnex_core::FlowNodeKind::LlmText { .. }
        | pnex_core::FlowNodeKind::PageWrite { .. }
        | pnex_core::FlowNodeKind::PagePdf { .. }
        | pnex_core::FlowNodeKind::TextDecide { .. }
        // Rejected messages surface here (O66, O69).
        | pnex_core::FlowNodeKind::Calc { .. }
        | pnex_core::FlowNodeKind::DeviceWrite { .. } => {
            cx.display_values.read().get(&node.id).cloned()
        }
        _ => None,
    };
    // Status badges (D103) carry their text/detail/colour in `status`.
    let display_badge = display_badge.map(|mut b| {
        if let Some(st) = &b.status {
            b.label = debug::status_label(st);
            b.pretty = Some(debug::status_pretty(st));
        }
        b
    });
    let (badge_fill, badge_stroke, badge_text) =
        match display_badge.as_ref().and_then(|b| b.status.as_ref()) {
            Some(st) => match st.level {
                pnex_core::vision::NodeStatusLevel::Ok => (
                    "var(--color-emerald-50, #ecfdf5)",
                    "var(--color-emerald-500, #10b981)",
                    "var(--color-emerald-700, #047857)",
                ),
                pnex_core::vision::NodeStatusLevel::Warn => (
                    "var(--color-amber-50, #fffbeb)",
                    "var(--color-amber-500, #f59e0b)",
                    "var(--color-amber-700, #b45309)",
                ),
                pnex_core::vision::NodeStatusLevel::Error => (
                    "var(--color-red-50, #fef2f2)",
                    "var(--color-red-500, #ef4444)",
                    "var(--color-red-700, #b91c1c)",
                ),
            },
            None => (
                "var(--color-cyan-50, #ecfeff)",
                "var(--color-cyan-500, #06b6d4)",
                "var(--color-cyan-700, #0e7490)",
            ),
        };
    // Ports de sortie précalculés (pas de `let` dans rsx) : un par pin +
    // « tout » pour device-read, un seul pour tous les autres kinds. Labels
    // affichés à droite des ancres (port « tout » inclus). Hauteur dyna-
    // mique : le nœud grandit avec le nombre de ports (pas PORT_PITCH).
    let out_count = state::output_count_of(&node);
    let accepts_input = geometry::accepts_input(&node);
    let node_h = geometry::node_height_for(&node);
    // Libellés de ports affichés dès qu'il y a plusieurs ports, et toujours
    // pour une fonction (une sortie nommée unique doit montrer son nom ;
    // une fonction sans @output garde son port unique non libellé).
    let show_port_labels = out_count > 1
        || matches!(&node.kind, pnex_core::FlowNodeKind::PnexFunction { .. })
        || matches!(&node.kind, pnex_core::FlowNodeKind::JsonSplit { config } if !config.keys.is_empty());
    let port_rows: Vec<(usize, f64, String, String)> = match &node.kind {
        pnex_core::FlowNodeKind::DeviceRead { config } => {
            // y RELATIF au nœud : les cercles sont rendus dans le <g> translaté
            // (port_out_at donne l'absolu — pour la couche wires seulement).
            let rel_y = |port: usize| node_h * (port + 1) as f64 / (out_count + 1) as f64;
            let mut rows: Vec<(usize, f64, String, String)> = config
                .pins
                .iter()
                .enumerate()
                .map(|(i, pin)| (i, rel_y(i), pin.clone(), node.id.clone()))
                .collect();
            rows.push((
                config.pins.len(),
                rel_y(config.pins.len()),
                t!("flow-port-device-name").to_string(),
                node.id.clone(),
            ));
            rows
        }
        pnex_core::FlowNodeKind::DeviceWrite { .. } => {
            let rel_y = |port: usize| node_h * (port + 1) as f64 / (out_count + 1) as f64;
            vec![
                (0, rel_y(0), String::new(), node.id.clone()),
                (
                    1,
                    rel_y(1),
                    t!("flow-port-device-name").to_string(),
                    node.id.clone(),
                ),
            ]
        }
        pnex_core::FlowNodeKind::PnexFunction { config } => {
            let rel_y = |port: usize| node_h * (port + 1) as f64 / (out_count + 1) as f64;
            geometry::function_output_labels(&config.outputs)
                .into_iter()
                .enumerate()
                .map(|(i, label)| (i, rel_y(i), label, node.id.clone()))
                .collect()
        }
        pnex_core::FlowNodeKind::JsonSplit { config } => {
            let rel_y = |port: usize| node_h * (port + 1) as f64 / (out_count + 1) as f64;
            geometry::split_output_labels(&config.keys)
                .into_iter()
                .enumerate()
                .map(|(i, label)| (i, rel_y(i), label, node.id.clone()))
                .collect()
        }
        pnex_core::FlowNodeKind::ControlSource { config } => {
            // One port per listened control, labelled by its key; a node
            // with no control yet still shows its (unlabelled) output.
            let rel_y = |port: usize| node_h * (port + 1) as f64 / (out_count + 1) as f64;
            if config.controls.is_empty() {
                vec![(0, rel_y(0), String::new(), node.id.clone())]
            } else {
                config
                    .controls
                    .iter()
                    .map(|id| {
                        let key = crate::api::controls::key_of(id);
                        crate::api::controls::short_key(&key).to_string()
                    })
                    .enumerate()
                    .map(|(i, label)| (i, rel_y(i), label, node.id.clone()))
                    .collect()
            }
        }
        pnex_core::FlowNodeKind::MemoryRead { config } => {
            // Port 0 = every key as one object, then one port per key.
            let rel_y = |port: usize| node_h * (port + 1) as f64 / (out_count + 1) as f64;
            std::iter::once(t!("flows-memory-port-all").to_string())
                .chain(config.keys.iter().cloned())
                .enumerate()
                .map(|(i, label)| (i, rel_y(i), label, node.id.clone()))
                .collect()
        }
        pnex_core::FlowNodeKind::CoolProp { config } => {
            // Port 0 = all outputs as one object, then one port per output,
            // then the phase port.
            let rel_y = |port: usize| node_h * (port + 1) as f64 / (out_count + 1) as f64;
            let mut labels = vec![t!("flows-coolprop-port-all").to_string()];
            labels.extend(
                config
                    .outputs
                    .iter()
                    .map(|o| geometry::coolprop_quantity_label(o, config.output_unit(o))),
            );
            if config.include_phase {
                labels.push(t!("flows-coolprop-port-phase").to_string());
            }
            labels
                .into_iter()
                .enumerate()
                .map(|(i, label)| (i, rel_y(i), label, node.id.clone()))
                .collect()
        }
        pnex_core::FlowNodeKind::PagePdf { .. } => {
            // Port 0 = PDF ready, port 1 = AI version awaiting approval.
            let rel_y = |port: usize| node_h * (port + 1) as f64 / (out_count + 1) as f64;
            [t!("flow-port-pdf-ready"), t!("flow-port-pdf-approval")]
                .into_iter()
                .enumerate()
                .map(|(i, label)| (i, rel_y(i), label.to_string(), node.id.clone()))
                .collect()
        }
        pnex_core::FlowNodeKind::VisionTrack { .. } => {
            // Port 0 = current tracks (→ zones), port 1 = appear/disappear.
            let rel_y = |port: usize| node_h * (port + 1) as f64 / (out_count + 1) as f64;
            [t!("flow-port-tracks"), t!("flow-port-track-events")]
                .into_iter()
                .enumerate()
                .map(|(i, label)| (i, rel_y(i), label.to_string(), node.id.clone()))
                .collect()
        }
        pnex_core::FlowNodeKind::Anomaly { .. }
        | pnex_core::FlowNodeKind::Forecast { .. }
        | pnex_core::FlowNodeKind::Weather { .. }
        | pnex_core::FlowNodeKind::SiteRecipe { .. } => {
            let rel_y = |port: usize| node_h * (port + 1) as f64 / (out_count + 1) as f64;
            geometry::predict_output_labels(&node.kind)
                .into_iter()
                .enumerate()
                .map(|(i, label)| (i, rel_y(i), label, node.id.clone()))
                .collect()
        }
        _ => vec![(0usize, node_h / 2.0, String::new(), node.id.clone())],
    };
    // Labeled input anchors (one per row — device-write output pins, function
    // declared inputs). The engine has a single input: the wire lands on the
    // row closest to the source (`port_in_near`); real routing stays payload
    // driven. Anchors are grabbable: a drag started on one wires in
    // **reverse** (Node-RED school — the hovered node becomes the source).
    let in_anchor_rows: Vec<(f64, String, String, String)> = match &node.kind {
        pnex_core::FlowNodeKind::DeviceWrite { config } => {
            let rows = geometry::input_anchor_rows(&node, node_h);
            let pins = geometry::write_anchor_labels(config);
            rows.into_iter()
                .zip(pins)
                .map(|(y, pin)| (y, pin.clone(), pin, node.id.clone()))
                .collect()
        }
        pnex_core::FlowNodeKind::PnexFunction { config } => {
            let rows = geometry::input_anchor_rows(&node, node_h);
            let names: Vec<String> = config.inputs.iter().map(|i| i.name.clone()).collect();
            rows.into_iter()
                .zip(names)
                .map(|(y, name)| (y, name.clone(), name, node.id.clone()))
                .collect()
        }
        pnex_core::FlowNodeKind::JsonMerge { config } => {
            let rows = geometry::input_anchor_rows(&node, node_h);
            rows.into_iter()
                .zip(config.inputs.iter().cloned())
                .map(|(y, name)| (y, name.clone(), name, node.id.clone()))
                .collect()
        }
        pnex_core::FlowNodeKind::PnexNotify { config } => {
            let rows = geometry::input_anchor_rows(&node, node_h);
            rows.into_iter()
                .zip(geometry::notify_input_labels(config))
                .map(|(y, name)| (y, name.clone(), name, node.id.clone()))
                .collect()
        }
        pnex_core::FlowNodeKind::CoolProp { config } => {
            // Pin = input key (routing), label = quantity + unit.
            let rows = geometry::input_anchor_rows(&node, node_h);
            let inputs = [
                (
                    config.v1_key.clone(),
                    geometry::coolprop_quantity_label(&config.input1, &config.unit1),
                ),
                (
                    config.v2_key.clone(),
                    geometry::coolprop_quantity_label(&config.input2, &config.unit2),
                ),
            ];
            rows.into_iter()
                .zip(inputs)
                .map(|(y, (pin, label))| (y, pin, label, node.id.clone()))
                .collect()
        }
        _ => vec![],
    };
    // Text zone of the node (title + subtitle): when port labels are shown,
    // the right edge holds the output label column and the left edge the
    // input anchor labels — reserve both (~5 px/char at font 9, plus pad
    // and a breathing gap) and center the text in what remains. Clamped so
    // long labels can't collapse the zone.
    let right_reserve = match port_rows.iter().map(|r| r.2.chars().count()).max() {
        Some(n) if show_port_labels && n > 0 => (n as f64 * 5.0 + 18.0).clamp(36.0, 92.0),
        _ => 0.0,
    };
    let left_reserve = match in_anchor_rows.iter().map(|r| r.2.chars().count()).max() {
        Some(n) if n > 0 => (n as f64 * 5.0 + 14.0).clamp(0.0, 60.0),
        _ => 0.0,
    };
    let text_cx = (left_reserve + geometry::NODE_W - right_reserve) / 2.0;
    // Subtitle truncated to the zone (~6 px/char at font 10, 5 px made it
    // touch the port labels) so it never runs under them.
    let max_chars =
        ((geometry::NODE_W - left_reserve - right_reserve - 8.0) / 6.0).max(6.0) as usize;
    let subtitle = if subtitle.chars().count() > max_chars {
        let short: String = subtitle.chars().take(max_chars.saturating_sub(1)).collect();
        format!("{short}…")
    } else {
        subtitle
    };
    // Capturé par chaque closure (le composant capture `node` une seule fois).
    let node_id = node.id.clone();
    // Un clone par closure qui s'en saisit (E0382 — pas de Copy sur String).
    let node_id_in_anchor = node_id.clone();
    // Badge live : pré-calculs pour les closures rsx (pas de `let` dans rsx,
    // et une seule closure peut prendre possession de la vue pretty).
    // The frame wraps the text (monospace 9 ≈ 5.4 px/char), centred under the
    // node; past two node widths the text is cut, the full one in a tooltip.
    let badge_full = display_badge.as_ref().map(|b| b.label.clone());
    let badge_label = badge_full.clone().map(|l| {
        let max = ((2.0 * geometry::NODE_W - 10.0) / 5.4) as usize;
        if l.chars().count() > max {
            format!("{}…", l.chars().take(max - 1).collect::<String>())
        } else {
            l
        }
    });
    let badge_w = badge_label.as_ref().map_or(0.0, |l| {
        (l.chars().count() as f64 * 5.4 + 10.0).max(geometry::NODE_W - 8.0)
    });
    let badge_x = (geometry::NODE_W - badge_w) / 2.0;
    let badge_title = badge_full.unwrap_or_default();
    let badge_clickable = display_badge
        .as_ref()
        .map(|b| b.pretty.is_some())
        .unwrap_or(false);
    let badge_pretty_click = display_badge.as_ref().and_then(|b| b.pretty.clone());
    let node_id_badge = node_id.clone();

    rsx! {
        g {
            transform: "translate({pos.x} {pos.y})",
            onpointerdown: move |event| {
                event.stop_propagation();
                let Some(rect) = geometry::canvas_rect() else {
                    return;
                };
                let client = client_of(&event);
                let point = geometry::to_graph(client, rect, cx.pan.cloned(), cx.zoom.cloned());
                let grab = (point.0 - pos.x, point.1 - pos.y);
                cx.selected_node.set(Some(node_id.clone()));
                cx.interaction
                    .set(Interaction::Dragging {
                        id: node_id.clone(),
                        grab,
                        rect,
                    });
            },
            rect {
                width: "{geometry::NODE_W}",
                height: "{node_h}",
                rx: "8",
                fill: "{fill}",
                stroke: "{stroke}",
                "stroke-width": "2",
                style: "cursor: grab",
            }
            // Titre + sous-titre centrés verticalement (20/36 px aux 48 px
            // historiques — inchangés pour un nœud mono-port).
            text {
                x: "{text_cx}",
                y: "{node_h / 2.0 - 4.0}",
                "text-anchor": "middle",
                "font-size": "12",
                "font-weight": "600",
                fill: "var(--color-gray-800, #1f2937)",
                "pointer-events": "none",
                {title}
            }
            text {
                x: "{text_cx}",
                y: "{node_h / 2.0 + 12.0}",
                "text-anchor": "middle",
                "font-size": "10",
                fill: "var(--color-gray-500, #6b7280)",
                "pointer-events": "none",
                {subtitle}
            }
            // Input port of plain single-input nodes (device-write omits it
            // — its pin rows carry the visual; event sources have none).
            // Grabbable like the pin rows: drag from the anchor wires in
            // reverse.
            if in_anchor_rows.is_empty() && accepts_input {
                circle {
                    cx: "0",
                    cy: "{node_h / 2.0}",
                    r: "10",
                    fill: "transparent",
                    onpointerdown: move |event| {
                        event.stop_propagation();
                        start_reverse_wiring(cx, node_id_in_anchor.clone(), None, event);
                    },
                }
                circle {
                    cx: "0",
                    cy: "{node_h / 2.0}",
                    r: "4",
                    fill: "{stroke}",
                    "pointer-events": "none",
                }
            }
            for (anchor_y, pin_for_closure, pin_label, row_node_id) in in_anchor_rows.clone() {
                g {
                    // Halo invisible : cible de grab ≥ 20 px graphe (le point
                    // visible fait 8 px — trop fin au doigt comme à la souris).
                    circle {
                        cx: "0",
                        cy: "{anchor_y}",
                        r: "10",
                        fill: "transparent",
                        onpointerdown: move |event| {
                            event.stop_propagation();
                            start_reverse_wiring(
                                cx,
                                row_node_id.clone(),
                                Some(pin_for_closure.clone()),
                                event,
                            );
                        },
                    }
                    circle {
                        cx: "0",
                        cy: "{anchor_y}",
                        r: "4",
                        fill: "{stroke}",
                        "pointer-events": "none",
                    }
                    text {
                        x: "8",
                        y: "{anchor_y + 3.0}",
                        "font-size": "9",
                        fill: "var(--color-gray-500, #6b7280)",
                        "pointer-events": "none",
                        {pin_label}
                    }
                }
            }
            // Ports de sortie (amorcent le câblage avec leur index).
            for (port_index, port_y, port_label, port_node_id) in port_rows.clone() {
                g {
                    circle {
                        cx: "{geometry::NODE_W}",
                        cy: "{port_y}",
                        r: "7",
                        // Port colour = data type (O70). The ring takes the node
                        // background: a ring in the border colour merged with a
                        // same-colour type (video on a camera node) and made
                        // that port look bigger.
                        fill: super::wire_types::output_type(&cx.graph.read(), &port_node_id, port_index).color(),
                        stroke: "{fill}",
                        "stroke-width": "2",
                        style: "cursor: crosshair",
                        onpointerdown: move |event| {
                            event.stop_propagation();
                            let Some(rect) = geometry::canvas_rect() else {
                                return;
                            };
                            let client = client_of(&event);
                            let cursor = geometry::to_graph(client, rect, cx.pan.cloned(), cx.zoom.cloned());
                            cx.selected_node.set(Some(port_node_id.clone()));
                            cx.interaction
                                .set(Interaction::Wiring {
                                    from_id: port_node_id.clone(),
                                    port: port_index,
                                    cursor,
                                    hover_target: None,
                                    reverse: false,
                                    pin: None,
                                });
                        },
                    }
                    if !port_label.is_empty() && show_port_labels {
                        text {
                            x: "{geometry::NODE_W - 12.0}",
                            y: "{port_y + 3.0}",
                            "text-anchor": "end",
                            "font-size": "9",
                            fill: "var(--color-gray-500, #6b7280)",
                            "pointer-events": "none",
                            {port_label}
                        }
                    }
                }
            }
            // Badge live de la sonde (pastille sous le nœud) : raccourci
            // tronqué ; objet/tableau JSON → pastille cliquable, la vue
            // pretty s'ouvre en modal (le texte SVG ne plie pas au
            // multi-ligne).
            if let Some(label) = badge_label {
                g {
                    onpointerdown: move |event| {
                        // Le clic d'ouverture ne doit pas amorcer un pan/drag
                        // du fond (badge scalaire : rien à ouvrir, transparent
                        // aux gestes comme avant).
                        if badge_clickable {
                            event.stop_propagation();
                        }
                    },
                    // `pointerup`, not `click`: touches on the canvas get no
                    // click (see `pinch.rs`, ghost click suppression).
                    onpointerup: move |event| {
                        if let Some(pretty) = badge_pretty_click.clone() {
                            event.stop_propagation();
                            cx.expanded_display.set(Some((node_id_badge.clone(), pretty)));
                        }
                    },
                    title { "{badge_title}" }
                    rect {
                        x: "{badge_x}",
                        y: "{node_h + 4.0}",
                        width: "{badge_w}",
                        height: "16",
                        rx: "4",
                        fill: badge_fill,
                        stroke: badge_stroke,
                        "stroke-width": "1",
                        style: if badge_clickable { "cursor: pointer" } else { "pointer-events: none" },
                    }
                    text {
                        x: "{geometry::NODE_W / 2.0}",
                        y: "{node_h + 15.0}",
                        "text-anchor": "middle",
                        "font-size": "9",
                        "font-family": "monospace",
                        fill: badge_text,
                        "pointer-events": "none",
                        {label}
                    }
                }
            }
        }
    }
}

/// Anomaly / forecast subtitle: method or model wire name · input key
/// (raw config data, not localized — same school as the other subtitles).
fn predict_subtitle(method: &str, key: &str) -> String {
    if key.is_empty() {
        method.to_string()
    } else {
        format!("{method} · {key}")
    }
}

/// Vision-detect subtitle: model name (resolved by the inspector cache,
/// else a short id) · picked labels (`all` when unfiltered).
fn vision_subtitle(config: &pnex_core::vision::VisionDetectConfig) -> String {
    if config.model_id.is_empty() {
        return "—".into();
    }
    let model = crate::state::vision::model_name(&config.model_id)
        .unwrap_or_else(|| config.model_id.chars().take(8).collect());
    let labels = if config.labels.is_empty() {
        "all".to_string()
    } else {
        config.labels.join(", ")
    };
    let text = format!("{model} · {labels}");
    let short: String = text.chars().take(30).collect();
    if text.chars().count() > 30 {
        format!("{short}…")
    } else {
        short
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
        pnex_core::FlowNodeKind::Value { config } => {
            // Truncation safe (chars, not bytes — accented values).
            let short: String = config.value.to_string().chars().take(22).collect();
            if config.value.to_string().chars().count() > 22 {
                format!("{short}…")
            } else {
                short
            }
        }
        pnex_core::FlowNodeKind::DeviceRead { config } => {
            let n = config.pins.len();
            if config.device_id.is_empty() {
                "—".into()
            } else {
                // Compact on purpose: the canvas subtitle shares the node
                // width with the right-hand port-label column.
                let plural = if n == 1 { "" } else { "s" };
                format!(
                    "{} · {n} pin{plural} · {}s",
                    config.device_id, config.window_secs
                )
            }
        }
        pnex_core::FlowNodeKind::DeviceWrite { config } => {
            if config.device_id.is_empty() {
                "—".into()
            } else {
                let n = config.anchors().count();
                let plural = if n == 1 { "" } else { "s" };
                format!("{} · {n} output{plural}", config.device_id)
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
        pnex_core::FlowNodeKind::CoolProp { config } => {
            let name = if config.fluid_label.is_empty() {
                &config.fluid_spec
            } else {
                &config.fluid_label
            };
            let spec: String = name.chars().take(24).collect();
            if name.chars().count() > 24 {
                format!("{spec}…")
            } else {
                spec
            }
        }
        pnex_core::FlowNodeKind::Display { .. } => "display".into(),
        pnex_core::FlowNodeKind::RegTtHeat { config }
        | pnex_core::FlowNodeKind::RegTtCool { config } => {
            if config.actuator_pin.is_empty() {
                "—".into()
            } else {
                format!("{} → {}", config.setpoint, config.actuator_pin)
            }
        }
        pnex_core::FlowNodeKind::RegPid { config } => {
            if config.actuator_pin.is_empty() {
                "—".into()
            } else {
                format!(
                    "{} → {} · Kp{} · {} s",
                    config.setpoint, config.actuator_pin, config.kp, config.cycle_time_secs
                )
            }
        }
        pnex_core::FlowNodeKind::PnexNotify { config } => {
            let n = config.channel_ids.len();
            let vars = config.template_vars.len();
            if config.template_id.is_nil() {
                "template ?".into()
            } else if n == 0 {
                "channel ?".into()
            } else {
                // Compact English, like the device-read subtitle.
                let plural = if n == 1 { "" } else { "s" };
                if vars > 0 {
                    format!("{n} channel{plural}, {vars} var")
                } else {
                    format!("{n} channel{plural}")
                }
            }
        }
        pnex_core::FlowNodeKind::HttpFetch { config } => {
            if config.url.is_empty() {
                "—".into()
            } else {
                let short: String = config.url.chars().take(24).collect();
                if config.url.chars().count() > 24 {
                    format!("{short}…")
                } else {
                    short
                }
            }
        }
        pnex_core::FlowNodeKind::PnexFunction { config } => {
            if config.function_id == 0 {
                "—".into()
            } else {
                format!("{} v{}", config.function_name, config.version_number)
            }
        }
        pnex_core::FlowNodeKind::JsonSplit { .. } => "split".into(),
        pnex_core::FlowNodeKind::JsonMerge { config } => {
            format!("key \"{}\"", config.default_key)
        }
        pnex_core::FlowNodeKind::CameraSource { config } => {
            let slug = if config.source.is_device() {
                &config.device_id
            } else {
                &config.stream
            };
            if slug.is_empty() {
                "—".into()
            } else if config.max_fps > 0.0 {
                format!("{slug} · ≤{} fps", config.max_fps)
            } else {
                slug.clone()
            }
        }
        pnex_core::FlowNodeKind::VideoRecord { config } => {
            // Compact data summary: segment length · retention (∞ = forever).
            if config.retention_days == 0 {
                format!("{} s · ∞", config.segment_secs)
            } else {
                format!("{} s · {} d", config.segment_secs, config.retention_days)
            }
        }
        pnex_core::FlowNodeKind::VisionDetect { config } => vision_subtitle(config),
        // Interval · crop.
        pnex_core::FlowNodeKind::VisionCaption { config } => {
            let crop = if config.crop_best { " · crop" } else { "" };
            format!("1 / {} s{crop}", config.min_interval_secs)
        }
        // Question ids, or a dash until one is written.
        pnex_core::FlowNodeKind::TextDecide { config } => {
            let ids: Vec<&str> = config.questions.iter().map(|q| q.id.as_str()).collect();
            let text = if ids.is_empty() {
                "—".to_string()
            } else {
                ids.join(", ")
            };
            let short: String = text.chars().take(30).collect();
            if text.chars().count() > 30 {
                format!("{short}…")
            } else {
                text
            }
        }
        // Source · approval.
        pnex_core::FlowNodeKind::PagePdf { config } => {
            let src = match config.source {
                pnex_core::pages::PagePdfSource::Message => "msg",
                pnex_core::pages::PagePdfSource::LatestSendable => "latest",
            };
            if config.require_approval {
                format!("{src} · ✓")
            } else {
                src.to_string()
            }
        }
        // New page title template, or "+ page" in append mode.
        pnex_core::FlowNodeKind::PageWrite { config } => match config.mode {
            pnex_core::pages::PageWriteMode::NewPage => config.title.clone(),
            pnex_core::pages::PageWriteMode::Append => "+ …".to_string(),
        },
        // Start of the instruction.
        pnex_core::FlowNodeKind::LlmText { config } => {
            let text: String = config.prompt.chars().take(28).collect();
            if config.prompt.chars().count() > 28 {
                format!("{text}…")
            } else {
                text
            }
        }
        // Window · language.
        pnex_core::FlowNodeKind::MediaFuse { config } => {
            let lang = if config.language.is_empty() {
                "auto"
            } else {
                config.language.as_str()
            };
            format!("{} s · {lang}", config.window_secs)
        }
        // High score · lost delay.
        pnex_core::FlowNodeKind::VisionTrack { config } => {
            format!("≥ {} · {} s", config.high_score, config.lost_after_secs)
        }
        // Zone names, or a dash until one is drawn.
        pnex_core::FlowNodeKind::VisionZone { config } => {
            let names: Vec<&str> = config.zones.iter().map(|z| z.name.as_str()).collect();
            let text = if names.is_empty() {
                "—".to_string()
            } else {
                names.join(", ")
            };
            let short: String = text.chars().take(30).collect();
            if text.chars().count() > 30 {
                format!("{short}…")
            } else {
                text
            }
        }
        pnex_core::FlowNodeKind::EventLog { config } => {
            // Normalized stream name (what the events page lists) · level.
            let stream =
                pnex_core::events::event_stream_name(&config.stream).unwrap_or_else(|| "—".into());
            format!("{stream} · {}", config.level.wire())
        }
        pnex_core::FlowNodeKind::MemoryWrite { config } => {
            // key · lifetime
            let key = if config.key.is_empty() {
                "—"
            } else {
                config.key.as_str()
            };
            format!(
                "{key} · {}",
                pnex_core::memory::format_duration_secs(config.ttl_secs)
            )
        }
        pnex_core::FlowNodeKind::ControlSource { config } => match config.controls.as_slice() {
            [] => "—".into(),
            [one] => crate::api::controls::key_of(one),
            [first, rest @ ..] => {
                format!("{} +{}", crate::api::controls::key_of(first), rest.len())
            }
        },
        pnex_core::FlowNodeKind::ActionTrigger { config } => {
            if config.action.is_empty() {
                "—".into()
            } else {
                format!("{} · {}", config.object_type, config.action)
            }
        }
        pnex_core::FlowNodeKind::OntologyQuery { config } => match config.object_type.as_str() {
            "" => "—".into(),
            t => format!("{t} · {}", config.filters.len()),
        },
        pnex_core::FlowNodeKind::ObjectUpsert { config } => match config.object_type.as_str() {
            "" => "—".into(),
            t => format!("{t} · {}", config.key),
        },
        pnex_core::FlowNodeKind::LinkUpsert { config } => match config.link_type.as_str() {
            "" => "—".into(),
            l if config.close => format!("✕ {l}"),
            l => l.to_string(),
        },
        pnex_core::FlowNodeKind::Ingest { config } => {
            use pnex_core::ingest::IngestStep as S;
            let s = match config.step {
                S::SourceResolve if config.dataset.is_empty() => config.url.clone(),
                S::SourceResolve => config.resource_match.clone(),
                S::ArchiveExtract => config.entries.join(", "),
                S::TableInspect => config.entry.clone(),
                S::RowsFilter => format!("{} · {}", config.entry, config.column),
                _ => String::new(),
            };
            match s.trim_matches([' ', '·']) {
                "" => "—".into(),
                t => t.to_string(),
            }
        }
        pnex_core::FlowNodeKind::MediaSource { config } => match config.streams.as_slice() {
            [] => "—".into(),
            [one] => one.clone(),
            [first, rest @ ..] => format!("{first} +{}", rest.len()),
        },
        pnex_core::FlowNodeKind::TopicClassify { config } => format!("v{}", config.version),
        pnex_core::FlowNodeKind::RangeUpsert { config } => {
            format!("{} · {}", config.scope_kind, config.origin)
        }
        // The rule's name lives in the org: the subtitle shows its parameters.
        pnex_core::FlowNodeKind::SiteRecipe { config } if config.recipe_id.is_empty() => "—".into(),
        pnex_core::FlowNodeKind::SiteRecipe { config } => config
            .params
            .values()
            .cloned()
            .collect::<Vec<_>>()
            .join(", "),
        pnex_core::FlowNodeKind::Weather { config } => format!(
            "{:.2}, {:.2} · {} min",
            config.latitude, config.longitude, config.interval_min
        ),
        pnex_core::FlowNodeKind::MemoryRead { config } => match config.keys.as_slice() {
            [] => "—".into(),
            [one] => one.clone(),
            [first, rest @ ..] => format!("{first} +{}", rest.len()),
        },
        pnex_core::FlowNodeKind::Anomaly { config } => {
            predict_subtitle(config.method.wire(), &config.key)
        }
        pnex_core::FlowNodeKind::Forecast { config } => {
            let base = predict_subtitle(config.model.wire(), &config.key);
            match config.threshold {
                Some(t) if config.direction == pnex_core::predictive::BreachDirection::Below => {
                    format!("{base} · ≤{t}")
                }
                Some(t) => format!("{base} · ≥{t}"),
                None => base,
            }
        }
        pnex_core::FlowNodeKind::Debug { .. } => "debug".into(),
    }
}
