use super::*;

/// Start anchor of the temporary wire (0,0 if the source is gone —
/// defensive case, the hit-test already excludes the source).
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

/// Start anchor of the temporary wire in **reverse** wiring: the node's
/// input anchor — targeted row (device-write / function) or middle of the
/// left edge (0,0 if the source is gone — defensive case).
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

/// pointerdown on an input anchor: selects the node and starts
/// **reverse** wiring (the target hovered on release becomes the source).
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

/// Client (screen px) → tuple.
pub(super) fn client_of(event: &PointerEvent) -> (f64, f64) {
    let point = event.client_coordinates();
    (point.x, point.y)
}

/// Background pointerdown: deselect + start of the pan.
pub(super) fn canvas_pointer_down(event: PointerEvent, mut cx: EditorCx) {
    cx.selected_node.set(None);
    cx.interaction.set(Interaction::Panning {
        start_client: client_of(&event),
        start_pan: cx.pan.cloned(),
    });
}

/// pointermove: acts according to the current gesture (the handler lives on
/// the root — the pointer can leave the node without breaking the drag).
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

/// pointerup: finalises any wiring, otherwise back to idle.
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
                // Row targeted on drop: the nearest **free** one (or already
                // on this source) — two sources dropped one after the other
                // land on two different rows; taking over a used
                // row only happens on a full board (explicit takeover).
                if let Some(label) = state::claim_input_row(graph, &target, &from_id, cursor.1) {
                    state::set_input_target(graph, &from_id, port, &target, label);
                }
            });
            cx.interaction.set(Interaction::Idle);
        }
        // Reverse wiring: the gesture was born on an input anchor, the hovered
        // target becomes the **source**. Targeted output port = row
        // nearest to the cursor (multi-port device-read), 0 for single-port;
        // the targeted input row stays `pin` (device-write only).
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
        _ => end_gesture(cx),
    }
}

/// Distance (graph px) a wire must travel before a miss is reported: a
/// plain click on an anchor stays silent.
const MISSED_WIRE_MIN: f64 = 24.0;

/// Back to idle; a wire released away from any input used to vanish
/// silently, it now says why (O58).
pub(super) fn end_gesture(mut cx: EditorCx) {
    let missed = match &*cx.interaction.peek() {
        Interaction::Wiring {
            from_id,
            port,
            cursor,
            hover_target: None,
            reverse,
            pin,
        } => {
            let graph = cx.graph.peek();
            let origin = if *reverse {
                temp_input_origin(&graph, from_id, pin.as_deref())
            } else {
                temp_origin(&graph, from_id, *port)
            };
            (cursor.0 - origin.0).hypot(cursor.1 - origin.1) > MISSED_WIRE_MIN
        }
        _ => false,
    };
    if missed {
        crate::state::toasts::info("flows-wire-missed");
    }
    cx.interaction.set(Interaction::Idle);
}

/// Wheel: zoom towards the cursor, bounded (`ZOOM_MIN/MAX`).
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
    // One notch ≈ 10 % — multiplicative for a constant feel.
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
