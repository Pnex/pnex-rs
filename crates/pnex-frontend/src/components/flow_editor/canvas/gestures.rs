use super::*;

/// Ancre de départ du câble temporaire (0,0 si la source a disparu — cas
/// défensif, le hit-test exclut déjà la source).
pub(super) fn temp_origin(graph: &pnex_core::FlowGraph, id: &str, port: usize) -> (f64, f64) {
    graph
        .nodes
        .iter()
        .find(|n| n.id == id)
        .and_then(|n| n.position)
        .map(|pos| {
            let node = graph.nodes.iter().find(|n| n.id == id);
            let count = node.map(state::output_count_of).unwrap_or(1);
            let h = node
                .map(geometry::node_height_for)
                .unwrap_or(geometry::NODE_H);
            geometry::port_out_at(pos, port, count, h)
        })
        .unwrap_or((0.0, 0.0))
}

/// Ancre de départ du câble temporaire en câblage **inverse** : l'ancre
/// d'entrée du nœud — rangée visée (device-write / fonction) ou milieu du
/// bord gauche (0,0 si la source a disparu — cas défensif).
pub(super) fn temp_input_origin(
    graph: &pnex_core::FlowGraph,
    id: &str,
    pin: Option<&str>,
) -> (f64, f64) {
    graph
        .nodes
        .iter()
        .find(|n| n.id == id)
        .and_then(|node| {
            let pos = node.position?;
            let h = geometry::node_height_for(node);
            match pin {
                Some(pin) => {
                    let labels = geometry::input_row_labels(node);
                    let row = geometry::input_anchor_rows(node, h);
                    labels
                        .iter()
                        .position(|p| p == pin)
                        .map(|i| (pos.x, pos.y + row[i]))
                }
                None => Some(geometry::port_in(pos, h)),
            }
        })
        .unwrap_or((0.0, 0.0))
}

/// pointerdown sur une ancre d'entrée : sélectionne le nœud et amorce le
/// câblage **inverse** (la cible survolée au relâcher devient la source).
pub(super) fn start_reverse_wiring(
    mut cx: EditorCx,
    node_id: String,
    pin: Option<String>,
    event: PointerEvent,
) {
    let Some(rect) = geometry::canvas_rect() else {
        return;
    };
    let client = client_of(&event);
    let cursor = geometry::to_graph(client, rect, cx.pan.cloned(), cx.zoom.cloned());
    cx.selected_node.set(Some(node_id.clone()));
    cx.interaction.set(Interaction::Wiring {
        from_id: node_id,
        port: 0,
        cursor,
        hover_target: None,
        reverse: true,
        pin,
    });
}

// ─────────────────────────────── Gestes ───────────────────────────────

/// Client (px écran) → tuple.
pub(super) fn client_of(event: &PointerEvent) -> (f64, f64) {
    let point = event.client_coordinates();
    (point.x, point.y)
}

/// pointerdown fond : désélection + début du pan.
pub(super) fn canvas_pointer_down(event: PointerEvent, mut cx: EditorCx) {
    cx.selected_node.set(None);
    cx.interaction.set(Interaction::Panning {
        start_client: client_of(&event),
        start_pan: cx.pan.cloned(),
    });
}

/// pointermove : agit selon le geste en cours (le handler vit sur le root —
/// le pointeur peut sortir du nœud sans casser le drag).
pub(super) fn canvas_pointer_move(event: PointerEvent, mut cx: EditorCx) {
    let client = client_of(&event);
    match cx.interaction.cloned() {
        Interaction::Idle => {}
        Interaction::Panning {
            start_client,
            start_pan,
        } => {
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
        Interaction::Wiring {
            from_id,
            port,
            reverse,
            pin,
            ..
        } => {
            let Some(rect) = geometry::canvas_rect() else {
                return;
            };
            let cursor = geometry::to_graph(client, rect, cx.pan.cloned(), cx.zoom.cloned());
            // Forward wiring lands on an input: a source node is no target.
            let hover_target = geometry::node_at(&cx.graph.peek(), cursor)
                .filter(|node| reverse || geometry::accepts_input(node))
                .map(|node| node.id.clone())
                .filter(|id| id != &from_id);
            cx.interaction.set(Interaction::Wiring {
                from_id,
                port,
                cursor,
                hover_target,
                reverse,
                pin,
            });
        }
    }
}

/// pointerup : finalise le câblage éventuel, sinon retour à l'idle.
pub(super) fn canvas_pointer_up(mut cx: EditorCx) {
    match cx.interaction.cloned() {
        Interaction::Wiring {
            from_id,
            port,
            cursor,
            hover_target: Some(target),
            reverse: false,
            ..
        } => {
            cx.update_graph(move |graph| {
                state::add_target(graph, &from_id, port, &target);
                // Ligne visée au drop : la plus proche **libre** (ou déjà à
                // cette source) — deux sources posées l'une après l'autre
                // atterrissent sur deux lignes différentes ; reprendre une
                // ligne prise n'arrive qu'à board pleine (reprise explicite).
                if let Some(label) = state::claim_input_row(graph, &target, &from_id, cursor.1) {
                    state::set_input_target(graph, &from_id, port, &target, label);
                }
            });
            cx.interaction.set(Interaction::Idle);
        }
        // Câblage inverse : le geste est né sur une ancre d'entrée, la cible
        // survolée devient la **source**. Port de sortie visé = rangée la
        // plus proche du curseur (device-read multi-ports), 0 en mono-port ;
        // la rangée d'entrée visée reste `pin` (device-write uniquement).
        Interaction::Wiring {
            from_id,
            cursor,
            hover_target: Some(target),
            reverse: true,
            pin,
            ..
        } => {
            cx.update_graph(move |graph| {
                let Some(source) = graph.nodes.iter().find(|n| n.id == target) else {
                    return;
                };
                let count = state::output_count_of(source);
                let h = geometry::node_height_for(source);
                let Some(src_pos) = source.position else {
                    return;
                };
                let port = geometry::port_out_near(src_pos, count, h, cursor.1);
                state::add_target(graph, &target, port, &from_id);
                if let Some(pin) = pin {
                    state::set_input_target(graph, &target, port, &from_id, pin);
                }
            });
            cx.interaction.set(Interaction::Idle);
        }
        _ => cx.interaction.set(Interaction::Idle),
    }
}

/// Molette : zoom vers le curseur, borné (`ZOOM_MIN/MAX`).
pub(super) fn canvas_wheel(event: WheelEvent, mut cx: EditorCx) {
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
    cx.pan.set(geometry::zoom_pan_towards(
        cx.pan.cloned(),
        old,
        new,
        screen,
    ));
    cx.zoom.set(new);
}
