//! Tour editor canvas: floor plan (blob `<image>`), scene pins,
//! links, gestures (pan / drag / selection / link mode). SVG **without**
//! `view_box` (1 unit = 1 CSS px, `flow_editor/canvas.rs` approach); the
//! document unit is the plan's native px.

use dioxus::prelude::*;
use dioxus_i18n::t;

use super::{geometry, state, Interaction, Select, TourEditorCx};
use crate::api;

/// Rendering palette (raw SVG attributes — parity with flow_editor).
const SCENE_FILL: &str = "var(--color-signal-50, #eff6ff)";
const SCENE_STROKE: &str = "var(--color-signal-500, #3b82f6)";
const SCENE_START_FILL: &str = "var(--color-emerald-50, #ecfdf5)";
const SCENE_START_STROKE: &str = "var(--color-emerald-500, #10b981)";
const SELECTED_STROKE: &str = "var(--color-signal-600, #2563eb)";
const LINK_STROKE: &str = "var(--color-slate-400, #94a3b8)";
const LINK_PENDING_STROKE: &str = "#f59e0b";
const STAIR_STROKE: &str = "#8b5cf6";

/// Drawable link: (id, source, target, stair?).
type DrawLink = (String, (f64, f64), (f64, f64), bool);

/// Render size of a floor plan: declared dimensions, else the size measured
/// from its image (same asset only), else the fallback.
fn plan_dims(
    plan: Option<&pnex_core::FloorPlan>,
    measured: Option<&(String, (f64, f64))>,
) -> (f64, f64) {
    let Some(plan) = plan else {
        return geometry::PLAN_FALLBACK;
    };
    let fallback = measured
        .filter(|(asset, _)| *asset == plan.media_asset_id)
        .map(|(_, size)| *size);
    geometry::plan_size(
        plan.width.or(fallback.map(|s| s.0)),
        plan.height.or(fallback.map(|s| s.1)),
    )
}

/// Canvas: background + plan of the active floor + scenes + intra-floor links.
#[component]
pub(crate) fn Canvas(cx: TourEditorCx, can_write: bool) -> Element {
    // Native size measured from the loaded plan image `(asset, (w, h))`:
    // written into the doc only when editable, kept here for display when
    // read-only (a viewer must never dirty the doc).
    let mut measured: Signal<Option<(String, (f64, f64))>> = use_signal(|| None);
    let doc = cx.doc.cloned();
    let floor_id = cx.active_floor.cloned();
    let pan = cx.pan.cloned();
    let zoom = cx.zoom.cloned();
    let selected = cx.selected.cloned();
    let link_mode = cx.link_mode.cloned();
    let link_from = cx.link_from.cloned();

    // Plan of the active floor (or none).
    let active_floor = doc.floors.iter().find(|f| f.id == floor_id).cloned();
    let plan_asset = active_floor
        .as_ref()
        .and_then(|f| f.plan.as_ref())
        .map(|p| p.media_asset_id.clone());
    let (plan_w, plan_h) = plan_dims(
        active_floor.as_ref().and_then(|f| f.plan.as_ref()),
        measured.read().as_ref(),
    );

    // Authenticated blob URL of the plan — the resource reads a **Memo** (plan
    // asset of the active floor): a by-value capture never restarts the
    // resource (dioxus pitfall #2784, see app.rs) — a plan set after the
    // first render was never shown.
    let plan_asset_memo = use_memo(move || {
        let doc = cx.doc.cloned();
        let floor_id = cx.active_floor.cloned();
        doc.floors
            .iter()
            .find(|f| f.id == floor_id)
            .and_then(|f| f.plan.as_ref())
            .map(|p| p.media_asset_id.clone())
    });
    let plan_url = use_resource(move || {
        let asset = plan_asset_memo.read().clone();
        async move {
            let asset = asset?;
            crate::util::media_blob_url(&api::media::content_path(&asset), Some("image/png")).await
        }
    });

    // Initial fit when the plan appears (web only — canvas_rect is
    // gated wasm32; natively the pan/zoom defaults stay).
    let mut fitted = use_signal(|| false);
    use_effect(move || {
        if fitted() || plan_url.value().read().is_none() {
            return;
        }
        let Some(rect) = geometry::canvas_rect() else {
            return;
        };
        // Current plan size read here (not the first render's capture): the
        // measured native size may land after the image.
        let (plan_w, plan_h) = {
            let doc = cx.doc.peek();
            let floor_id = cx.active_floor.peek();
            let plan = doc
                .floors
                .iter()
                .find(|f| f.id == *floor_id)
                .and_then(|f| f.plan.as_ref());
            plan_dims(plan, measured.peek().as_ref())
        };
        let ((pan_x, pan_y), zoom_value) =
            geometry::fit_transform(plan_w, plan_h, rect.2, rect.3, geometry::FIT_PADDING);
        cx.pan.set((pan_x, pan_y));
        cx.zoom.set(zoom_value);
        fitted.set(true);
    });
    // Refit on every change of the plan asset: the initial fit was
    // computed on the default dimensions (no plan) — the inspector
    // then allows declaring other dims, and an image arriving
    // afterwards must reframe (otherwise the plan stays out of view).
    use_effect(move || {
        plan_asset_memo.read();
        fitted.set(false);
    });
    // Plan without native size (picked before measurement, or legacy doc):
    // measure the loaded image and store its real width/height, then refit.
    use_effect(move || {
        let Some(Some(url)) = plan_url.value().read().clone() else {
            return;
        };
        let floor_id = cx.active_floor.cloned();
        let missing = cx
            .doc
            .read()
            .floors
            .iter()
            .find(|f| f.id == floor_id)
            .and_then(|f| f.plan.as_ref())
            .filter(|p| p.width.is_none() || p.height.is_none())
            .map(|p| p.media_asset_id.clone());
        let Some(asset) = missing else {
            return;
        };
        // Already measured for this asset (read-only path: the doc stays
        // without size, do not measure again on every doc change).
        if measured.peek().as_ref().is_some_and(|(a, _)| *a == asset) {
            return;
        }
        spawn(async move {
            let Some((w, h)) = geometry::image_natural_size(&url).await else {
                return;
            };
            measured.set(Some((asset.clone(), (w, h))));
            if !can_write {
                fitted.set(false);
                return;
            }
            cx.update_doc(|doc| {
                state::patch_plan(doc, &floor_id, |plan| {
                    if plan.media_asset_id == asset
                        && (plan.width.is_none() || plan.height.is_none())
                    {
                        plan.width = Some(w);
                        plan.height = Some(h);
                    }
                });
            });
            fitted.set(false);
        });
    });

    // Links whose two ends are on the active floor (render + hit).
    let visible_links: Vec<DrawLink> = doc
        .links
        .iter()
        .filter_map(|link| {
            let a = doc.scenes.iter().find(|s| s.id == link.from)?;
            let b = doc.scenes.iter().find(|s| s.id == link.to)?;
            if a.floor_id != floor_id || b.floor_id != floor_id {
                return None;
            }
            Some((
                link.id.clone(),
                (a.x, a.y),
                (b.x, b.y),
                link.kind == pnex_core::TOUR_LINK_STAIR,
            ))
        })
        .collect();

    let scenes: Vec<pnex_core::TourScene> = doc
        .scenes
        .iter()
        .filter(|s| s.floor_id == floor_id)
        .cloned()
        .collect();
    let start_scene = doc.start_scene.clone();

    // Precomputed pins (`let`s cannot be declared in the rsx body):
    // (scene, highlighted, start, fill, effective stroke).
    let scene_pins: Vec<(pnex_core::TourScene, bool, bool, &'static str, &'static str)> = scenes
        .iter()
        .map(|scene| {
            let is_selected = selected
                .as_ref()
                .is_some_and(|s| matches!(s, Select::Scene(id) if *id == scene.id));
            let is_link_from = link_from.as_deref() == Some(scene.id.as_str());
            let is_start = start_scene.as_deref() == Some(scene.id.as_str());
            let highlighted = is_selected || is_link_from;
            let (fill, base_stroke) = if is_start {
                (SCENE_START_FILL, SCENE_START_STROKE)
            } else {
                (SCENE_FILL, SCENE_STROKE)
            };
            let stroke = if highlighted {
                SELECTED_STROKE
            } else {
                base_stroke
            };
            (scene.clone(), highlighted, is_start, fill, stroke)
        })
        .collect();

    rsx! {
        div { class: "h-full w-full min-w-0 relative bg-gray-50 rounded-lg border border-gray-200 overflow-hidden",
            svg {
                id: "tour-canvas",
                class: "w-full h-full block touch-none outline-none select-none",
                tabindex: "0",
                onpointerdown: move |event| {
                    event.stop_propagation();
                    // Background: deselect + pan (never in link mode).
                    cx.selected.set(None);
                    let point = event.client_coordinates();
                    cx.interaction
                        .set(Interaction::Panning {
                            start_client: (point.x, point.y),
                            start_pan: cx.pan.cloned(),
                        });
                },
                onpointermove: move |event| canvas_pointer_move(event, cx),
                onpointerup: move |_| canvas_pointer_up(cx),
                onpointerleave: move |_| canvas_pointer_up(cx),
                onwheel: move |event| canvas_wheel(event, cx),
                onkeydown: move |event| {
                    // Escape: deselect (the inspector folds with it).
                    if event.key() == Key::Escape {
                        cx.selected.set(None);
                    }
                },
                g { transform: "translate({pan.0} {pan.1}) scale({zoom})",
                    // Plan (image stretched to the declared dimensions — fallback
                    // 2000×1000 if unknown; studio.md doc "still open").
                    if let Some(Some(Some(url))) = plan_url.value().read().as_ref().map(Some) {
                        image {
                            key: "plan-{plan_asset:?}",
                            href: "{url}",
                            x: "0",
                            y: "0",
                            width: "{plan_w}",
                            height: "{plan_h}",
                            preserve_aspect_ratio: "none",
                            "pointer-events": "none",
                        }
                    }
                    // Links (line + target dot; stair = dash-dot line).
                    for (link_id, a, b, is_stair) in visible_links.clone() {
                        line {
                            key: "link-{link_id}",
                            x1: "{a.0}",
                            y1: "{a.1}",
                            x2: "{b.0}",
                            y2: "{b.1}",
                            stroke: if is_stair { STAIR_STROKE } else { LINK_STROKE },
                            "stroke-width": "3",
                            "stroke-dasharray": if is_stair { "8 4" } else { "none" },
                            "pointer-events": "none",
                        }
                        circle {
                            cx: "{b.0}",
                            cy: "{b.1}",
                            r: "5",
                            fill: if is_stair { STAIR_STROKE } else { LINK_STROKE },
                            "pointer-events": "none",
                        }
                        // Wider transparent hit area for link selection.
                        line {
                            x1: "{a.0}",
                            y1: "{a.1}",
                            x2: "{b.0}",
                            y2: "{b.1}",
                            stroke: "transparent",
                            "stroke-width": "12",
                            "pointer-events": "stroke",
                            onpointerdown: move |event| {
                                event.stop_propagation();
                                cx.selected.set(Some(Select::Link(link_id.clone())));
                            },
                        }
                    }
                    // Pending link (link mode, source set) towards the centre
                    // of the plan — purely indicative.
                    if let Some(from_id) = link_from {
                        if let Some(from) = scenes.iter().find(|s| s.id == from_id) {
                            line {
                                x1: "{from.x}",
                                y1: "{from.y}",
                                x2: "{plan_w / 2.0}",
                                y2: "{plan_h / 2.0}",
                                stroke: LINK_PENDING_STROKE,
                                "stroke-width": "3",
                                "stroke-dasharray": "6 4",
                                "pointer-events": "none",
                            }
                        }
                    }
                    // Scenes: clickable pin + label (dedicated component —
                    // CanvasNode approach: gestures live in the component).
                    for (scene, highlighted, is_start, fill, stroke) in scene_pins {
                        ScenePin {
                            key: "scene-{scene.id}",
                            scene,
                            highlighted,
                            is_start,
                            fill,
                            stroke,
                            link_mode,
                            cx,
                        }
                    }
                }
            }
            // Floor without a plan: prompt banner (scenes can be placed after
            // import — the editor needs a plan to position them).
            if active_floor.as_ref().and_then(|f| f.plan.as_ref()).is_none() {
                // Top of the canvas: the centre belongs to the shell's empty hint.
                div { class: "absolute inset-x-0 top-20 flex justify-center pointer-events-none",
                    p { class: "rounded-lg border border-line bg-surface/90 px-4 py-2 text-sm text-slate",
                        {t!("studio-canvas-no-plan")}
                    }
                }
            }
        }
    }
}

/// Pin of a scene: clickable circle + label + "start" tag.
/// Dedicated component (`CanvasNode` approach): props are owned, the
/// gestures capture the scene by value without borrow conflicts.
#[component]
fn ScenePin(
    mut cx: TourEditorCx,
    scene: pnex_core::TourScene,
    highlighted: bool,
    is_start: bool,
    fill: &'static str,
    stroke: &'static str,
    link_mode: bool,
) -> Element {
    // Captured by the gesture closure (the component captures `scene`
    // once — CanvasNode approach: one named local clone per closure).
    let scene_for_down = scene.clone();
    rsx! {
        g {
            key: "scene-{scene.id}",
            cursor: if link_mode { "crosshair" } else { "grab" },
            onpointerdown: move |event| {
                event.stop_propagation();
                scene_pointer_down(
                    &scene_for_down,
                    cx,
                    (event.client_coordinates().x, event.client_coordinates().y),
                );
            },
            circle {
                cx: "{scene.x}",
                cy: "{scene.y}",
                r: "{geometry::SCENE_RADIUS}",
                fill: "{fill}",
                stroke: "{stroke}",
                "stroke-width": if highlighted { "4" } else { "2" },
            }
            text {
                x: "{scene.x}",
                y: "{scene.y + geometry::SCENE_RADIUS + 14.0}",
                "text-anchor": "middle",
                "font-size": "13",
                fill: "var(--color-gray-700, #374151)",
                "pointer-events": "none",
                {if scene.label.is_empty() { scene.id.clone() } else { scene.label.clone() }}
            }
            if is_start {
                text {
                    x: "{scene.x}",
                    y: "{scene.y - geometry::SCENE_RADIUS - 6.0}",
                    "text-anchor": "middle",
                    "font-size": "12",
                    fill: "var(--color-emerald-600, #10b981)",
                    "pointer-events": "none",
                    {t!("studio-scene-start-tag")}
                }
            }
        }
    }
}

/// pointerdown on a scene: link mode → sets the source/creates the link;
/// otherwise drag start (grab = cursor↔pin offset, measured here).
fn scene_pointer_down(scene: &pnex_core::TourScene, mut cx: TourEditorCx, client: (f64, f64)) {
    if cx.link_mode.cloned() {
        let target = scene.id.clone();
        if let Some(from) = cx.link_from.cloned() {
            if from != target {
                // Round-trip pair + auto angles (yaw = plan direction,
                // pitch = ±30° if different floors) — see state::connect_scenes.
                cx.update_doc(move |doc| {
                    state::connect_scenes(doc, &from, &target);
                });
            }
            cx.link_from.set(None);
        } else {
            cx.link_from.set(Some(target));
        }
        return;
    }
    cx.selected.set(Some(Select::Scene(scene.id.clone())));
    let Some(rect) = geometry::canvas_rect() else {
        return; // native: selection without drag
    };
    let point = geometry::to_plan(client, rect, cx.pan.cloned(), cx.zoom.cloned());
    cx.interaction.set(Interaction::Dragging {
        id: scene.id.clone(),
        grab: (point.0 - scene.x, point.1 - scene.y),
        rect,
    });
}

fn canvas_pointer_move(event: PointerEvent, mut cx: TourEditorCx) {
    let point = event.client_coordinates();
    let client = (point.x, point.y);
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
            let point = geometry::to_plan(client, rect, cx.pan.cloned(), cx.zoom.cloned());
            // No grid snap: the unit is the plan's native px (free).
            let pos = (point.0 - grab.0, point.1 - grab.1);
            cx.update_doc(move |doc| state::move_scene(doc, &id, pos));
        }
    }
}

fn canvas_pointer_up(mut cx: TourEditorCx) {
    cx.interaction.set(Interaction::Idle);
}

/// Wheel: zoom towards the cursor, bounded (flow_editor approach).
fn canvas_wheel(event: WheelEvent, mut cx: TourEditorCx) {
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
    let screen = (point.x - rect.0, point.y - rect.1);
    let zoom = cx.zoom.cloned();
    let new_zoom = (zoom * (if delta_y > 0.0 { 0.9 } else { 1.1 }))
        .clamp(geometry::ZOOM_MIN, geometry::ZOOM_MAX);
    let new_pan = geometry::zoom_pan_towards(cx.pan.cloned(), zoom, new_zoom, screen);
    cx.pan.set(new_pan);
    cx.zoom.set(new_zoom);
}
