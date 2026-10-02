//! Free canvas of the dashboard editor: transformed viewport div, pointer
//! and keyboard gesture handlers, and the widget/wire/anchor elements
//! rendered inside it (pointer-events school inherited from the flow
//! editor).

use super::*;

use crate::components::dashboard_widget::WidgetBody;

/// Le canvas : un div **transformé** (translate + scale) porte le
/// document ; les widgets sont des divs HTML absolus (rendu identique au
/// live), les traits et accroches dans un SVG en surcouche. Toute la
/// géométrie passe par `geometry::to_canvas` (1 unité = 1 px document).
#[component]
pub(super) fn CanvasView(cx: EditorCx) -> Element {
    let layout_snapshot = cx.layout.read().clone();
    let canvas = layout_snapshot.canvas.clone();
    let widgets = layout_snapshot.widgets.clone();
    let wires = layout_snapshot.wires.clone();
    let zoom = cx.zoom.cloned();
    let (pan_x, pan_y) = cx.pan.cloned();
    let tool = cx.tool.cloned();
    let selected = cx.selected.cloned();
    let wire_draft = cx.wire_draft.cloned();
    let interaction = cx.interaction.cloned();
    let live = cx.live.read().clone();
    let bg = canvas
        .background
        .clone()
        .unwrap_or_else(|| "#f8fafc".into());

    rsx! {
        div {
            id: "dashboard-canvas",
            class: "relative h-full w-full overflow-hidden bg-gray-100",
            tabindex: "0",
            onpointerdown: move |event| canvas_pointer_down(event, cx),
            onpointermove: move |event| canvas_pointer_move(event, cx),
            onpointerup: move |event| canvas_pointer_up(event, cx),
            onpointerleave: move |_| {
                // Geste orphelin (curseur sorti) : annulation propre.
                cx.interaction.set(Interaction::Idle);
                cx.drag_template.set(None);
            },
            onwheel: move |event| canvas_wheel(event, cx),
            onkeydown: move |event| key_handler(event, cx),

            div { style: "transform: translate({pan_x}px, {pan_y}px) scale({zoom}); transform-origin: 0 0; width: {canvas.width}px; height: {canvas.height}px; background-color: {bg}; position: absolute;",
                // ── Widgets (divs absolus, sous les accroches)
                for w in &widgets {
                    WidgetElement {
                        key: "{w.id}",
                        w: w.clone(),
                        selected: selected.clone(),
                        live: live.clone(),
                        interaction: interaction.clone(),
                        cx,
                    }
                }
                // ── Traits + accroches (surcouche SVG)
                svg {
                    xmlns: "http://www.w3.org/2000/svg",
                    style: "position: absolute; inset: 0; width: 100%; height: 100%; pointer-events: none; overflow: visible;",
                    for wire in &wires {
                        WireElement {
                            key: "{wire.id}",
                            layout: layout_snapshot.clone(),
                            wire: wire.clone(),
                            selected: selected.clone(),
                            cx,
                        }
                    }
                    if tool == Tool::Wire {
                        for w in &widgets {
                            for side in [
                                pnex_core::WireSide::Top,
                                pnex_core::WireSide::Bottom,
                                pnex_core::WireSide::Left,
                                pnex_core::WireSide::Right,
                            ]
                            {
                                AnchorElement {
                                    key: "{w.id}-{side:?}",
                                    w: w.clone(),
                                    side,
                                    wire_draft: wire_draft.clone(),
                                    cx,
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

fn client_of(event: &PointerEvent) -> (f64, f64) {
    let point = event.client_coordinates();
    (point.x, point.y)
}

/// pointerdown fond : désélection + début du pan (école flow editor).
fn canvas_pointer_down(event: PointerEvent, mut cx: EditorCx) {
    event.stop_propagation();
    let client = client_of(&event);
    cx.selected.set(None);
    cx.interaction.set(Interaction::Panning {
        start: client,
        origin: cx.pan.cloned(),
    });
}

fn canvas_pointer_move(event: PointerEvent, mut cx: EditorCx) {
    let client = client_of(&event);
    match cx.interaction.cloned() {
        Interaction::Panning { start, origin } => {
            cx.pan.set((
                origin.0 + (client.0 - start.0),
                origin.1 + (client.1 - start.1),
            ));
        }
        Interaction::Dragging { id, grab } => {
            let Some(rect) = geometry::canvas_rect() else {
                return;
            };
            let (gx, gy) = geometry::to_canvas(client, rect, cx.pan.cloned(), cx.zoom.cloned());
            let x = (geometry::snap(gx - grab.0) as i64).max(0);
            let y = (geometry::snap(gy - grab.1) as i64).max(0);
            cx.layout.with_mut(|l| state::move_widget(l, &id, x, y));
        }
        Interaction::Resizing { id } => {
            let Some(rect) = geometry::canvas_rect() else {
                return;
            };
            // Snapshot SANS garder le garde de lecture vivant pendant la
            // mutation (RefCell : read + with_mut simultanés = panic).
            let (wx, wy, cw, ch) = {
                let l = cx.layout.read();
                let Some(w) = geometry::widget_of(&l, &id) else {
                    return;
                };
                (w.x, w.y, l.canvas.width, l.canvas.height)
            };
            let (gx, gy) = geometry::to_canvas(client, rect, cx.pan.cloned(), cx.zoom.cloned());
            let nw = (geometry::snap(gx - wx as f64) as i64)
                .max(pnex_core::WIDGET_MIN)
                .min(cw - wx);
            let nh = (geometry::snap(gy - wy as f64) as i64)
                .max(pnex_core::WIDGET_MIN)
                .min(ch - wy);
            cx.layout.with_mut(|l| state::resize_widget(l, &id, nw, nh));
        }
        Interaction::Idle => {}
    }
}

fn canvas_pointer_up(event: PointerEvent, mut cx: EditorCx) {
    // Pose d'un modèle draggué depuis la bibliothèque (le pointerup
    // determinant est celui du canvas ; la palette désarme au sien).
    if let Some(tpl) = cx.drag_template.cloned() {
        let Some(rect) = geometry::canvas_rect() else {
            return;
        };
        let client = client_of(&event);
        let (gx, gy) = geometry::to_canvas(client, rect, cx.pan.cloned(), cx.zoom.cloned());
        let current = cx.layout.read().clone();
        let x = (geometry::snap(gx) as i64).clamp(0, (current.canvas.width - 40).max(0));
        let y = (geometry::snap(gy) as i64).clamp(0, (current.canvas.height - 40).max(0));
        cx.history.with_mut(|h| h.push(&current));
        cx.counter.with_mut(|c| *c += 1);
        let id = state::next_id("w", cx.counter.cloned());
        let template = tpl.config.clone();
        cx.layout
            .with_mut(|l| state::place_widget(l, &template, id.clone(), x, y));
        cx.selected.set(Some(Selection::Widget(id)));
        cx.drag_template.set(None);
        cx.interaction.set(Interaction::Idle);
        return;
    }
    cx.interaction.set(Interaction::Idle);
}

/// Molette : zoom vers le curseur (copie `flow_editor::canvas_wheel`).
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
    let cursor = (point.x - rect.0, point.y - rect.1);
    let factor = if delta_y < 0.0 { 1.1 } else { 1.0 / 1.1 };
    let (z, pan) = geometry::zoom_pan_towards(cx.zoom.cloned(), cx.pan.cloned(), cursor, factor);
    cx.zoom.set(z);
    cx.pan.set(pan);
}

/// Clavier : Delete/Backspace supprime la sélection, Échap désarme
/// (undo/redo = boutons de la barre).
fn key_handler(event: KeyboardEvent, mut cx: EditorCx) {
    let key = event.key();
    if key == Key::Delete || key == Key::Backspace {
        let Some(sel) = cx.selected.cloned() else {
            return;
        };
        let current = cx.layout.read().clone();
        cx.history.with_mut(|h| h.push(&current));
        match sel {
            Selection::Widget(id) => {
                cx.layout.with_mut(|l| state::delete_widget(l, &id));
            }
            Selection::Wire(id) => {
                cx.layout.with_mut(|l| state::delete_wire(l, &id));
            }
        }
        cx.selected.set(None);
    } else if key == Key::Escape {
        cx.selected.set(None);
        cx.wire_draft.set(None);
    }
}

#[component]
fn WidgetElement(
    w: pnex_core::Widget,
    selected: Option<Selection>,
    live: HashMap<String, Option<Vec<TelemetryPoint>>>,
    interaction: Interaction,
    mut cx: EditorCx,
) -> Element {
    let id = w.id.clone();
    let is_selected = matches!(&selected, Some(Selection::Widget(sel)) if *sel == w.id);
    // Symbols are drawings, not cards: no background, outline only on
    // hover / selection.
    let is_symbol = w.widget_type == "symbol";
    let frame = match (is_symbol, is_selected) {
        (false, true) => "bg-white shadow-sm rounded-lg ring-2 ring-blue-500",
        (false, false) => "bg-white shadow-sm rounded-lg ring-1 ring-gray-200 hover:ring-blue-300",
        (true, true) => "rounded ring-2 ring-blue-500",
        (true, false) => "rounded hover:ring-1 hover:ring-blue-300",
    };
    let busy = matches!(
        interaction,
        Interaction::Dragging { .. } | Interaction::Resizing { .. } | Interaction::Panning { .. }
    );
    // Valeurs de la source primaire (photo figée en édition).
    let points = w
        .source
        .first()
        .and_then(|s| live.get(&s.series_key()))
        .cloned()
        .flatten();
    // Handler kept out of rsx!: dx fmt mis-splices long handler bodies.
    let w_down = w.clone();
    let on_pointer_down = {
        let id = id.clone();
        move |event: PointerEvent| {
            event.stop_propagation();
            if cx.tool.cloned() != Tool::Select {
                return;
            }
            let Some(rect) = geometry::canvas_rect() else {
                return;
            };
            let point = client_of(&event);
            let (gx, gy) = geometry::to_canvas(point, rect, cx.pan.cloned(), cx.zoom.cloned());
            cx.selected.set(Some(Selection::Widget(id.clone())));
            if !is_selected {
                let current = cx.layout.read().clone();
                cx.history.with_mut(|h| h.push(&current));
            }
            if geometry::resize_handle_at(&w_down, (gx, gy), 8.0) {
                cx.interaction.set(Interaction::Resizing { id: id.clone() });
            } else {
                let grab = (gx - w_down.x as f64, gy - w_down.y as f64);
                cx.interaction.set(Interaction::Dragging {
                    id: id.clone(),
                    grab,
                });
            }
        }
    };

    rsx! {
        div {
            style: "position: absolute; left: {w.x}px; top: {w.y}px; width: {w.w}px; height: {w.h}px;",
            class: "{frame} select-none",
            onpointerdown: on_pointer_down,
            WidgetBody { widget: w.clone(), points, values: Some(live.clone()) }
            if is_selected && !busy {
                div { class: "absolute -bottom-1 -right-1 h-3 w-3 rounded-sm bg-blue-500 cursor-se-resize" }
            }
        }
    }
}

#[component]
fn WireElement(
    layout: DashboardLayout,
    wire: pnex_core::Wire,
    selected: Option<Selection>,
    mut cx: EditorCx,
) -> Element {
    let Some(path) = geometry::wire_path(
        &layout,
        &wire.from.widget_id,
        wire.from.side,
        &wire.to.widget_id,
        wire.to.side,
    ) else {
        return rsx! {};
    };
    let id = wire.id.clone();
    let is_selected = matches!(&selected, Some(Selection::Wire(sel)) if *sel == wire.id);
    let stroke = if is_selected { "#2563eb" } else { "#94a3b8" };
    rsx! {
        g {
            style: "pointer-events: stroke; cursor: pointer;",
            onpointerdown: move |event| {
                event.stop_propagation();
                if cx.tool.cloned() == Tool::Select {
                    cx.selected.set(Some(Selection::Wire(id.clone())));
                }
            },
            path {
                d: "{path}",
                fill: "none",
                stroke: "{stroke}",
                "stroke-width": "6",
                "stroke-opacity": "0",
                "stroke-linecap": "round",
            }
            path {
                d: "{path}",
                fill: "none",
                stroke: "{stroke}",
                "stroke-width": "2",
                "stroke-dasharray": "6 4",
                "stroke-linecap": "round",
            }
        }
    }
}

#[component]
fn AnchorElement(
    w: pnex_core::Widget,
    side: pnex_core::WireSide,
    wire_draft: Option<WireEndpoint>,
    mut cx: EditorCx,
) -> Element {
    let (ax, ay) = geometry::anchor_point(&w, side);
    let endpoint = WireEndpoint {
        widget_id: w.id.clone(),
        side,
    };
    let armed = matches!(&wire_draft, Some(d) if d.widget_id == w.id && d.side == side);
    let fill = if armed { "#2563eb" } else { "#94a3b8" };
    rsx! {
        circle {
            cx: "{ax}",
            cy: "{ay}",
            r: "6",
            fill: "{fill}",
            stroke: "#ffffff",
            "stroke-width": "2",
            style: "pointer-events: all; cursor: crosshair;",
            onpointerdown: move |event| {
                event.stop_propagation();
                let Some(first) = cx.wire_draft.cloned() else {
                    cx.wire_draft.set(Some(endpoint.clone()));
                    return;
                };
                if first.widget_id == endpoint.widget_id {
                    // Re-clic sur la même accroche : désarmement.
                    cx.wire_draft.set(None);
                    return;
                }
                let current = cx.layout.read().clone();
                cx.history.with_mut(|h| h.push(&current));
                cx.counter.with_mut(|c| *c += 1);
                let id = state::next_id("t", cx.counter.cloned());
                cx.layout
                    .with_mut(|l| {
                        state::add_wire(
                            l,
                            pnex_core::Wire {
                                id,
                                from: first,
                                to: endpoint.clone(),
                            },
                        )
                    });
                cx.wire_draft.set(None);
            },
        }
    }
}
