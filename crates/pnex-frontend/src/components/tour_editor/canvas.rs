//! Canevas de l'éditeur de tour : plan d'étage (`<image>` blob), pins de
//! scènes, liens, gestes (pan / drag / sélection / mode lien). SVG **sans**
//! `view_box` (1 unité = 1 px CSS, école `flow_editor/canvas.rs`) ; l'unité
//! du document est le px natif du plan.

use dioxus::prelude::*;
use dioxus_i18n::t;

use super::{geometry, state, Interaction, Select, TourEditorCx};
use crate::api;

/// Palette de rendu (attributs SVG bruts — parité flow_editor).
const SCENE_FILL: &str = "#eff6ff";
const SCENE_STROKE: &str = "#3b82f6";
const SCENE_START_FILL: &str = "#ecfdf5";
const SCENE_START_STROKE: &str = "#10b981";
const SELECTED_STROKE: &str = "#2563eb";
const LINK_STROKE: &str = "#94a3b8";
const LINK_PENDING_STROKE: &str = "#f59e0b";
const STAIR_STROKE: &str = "#8b5cf6";

/// Lien dessinable : (id, source, cible, stair ?).
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

/// Canevas : fond + plan de l'étage actif + scènes + liens intra-étage.
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

    // Plan de l'étage actif (ou aucun).
    let active_floor = doc.floors.iter().find(|f| f.id == floor_id).cloned();
    let plan_asset = active_floor
        .as_ref()
        .and_then(|f| f.plan.as_ref())
        .map(|p| p.media_asset_id.clone());
    let (plan_w, plan_h) = plan_dims(
        active_floor.as_ref().and_then(|f| f.plan.as_ref()),
        measured.read().as_ref(),
    );

    // Blob URL authentifié du plan — la resource relit un **Memo** (asset du
    // plan de l'étage actif) : une capture en valeur ne redémarre jamais la
    // resource (piège dioxus #2784, cf. app.rs) — le plan posé après le
    // premier rendu ne s'affichait jamais.
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

    // Fit initial quand le plan apparaît (web uniquement — canvas_rect est
    // gated wasm32 ; en natif les défauts de pan/zoom restent).
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
    // Refit à chaque changement d'asset du plan : l'ajustement initial a été
    // calculé sur les dimensions par défaut (plan absent) — l'inspector
    // permet ensuite de déclarer d'autres dims, et l'arrivée d'une image
    // après coup doit recadrer (sinon le plan reste hors-champ).
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

    // Liens dont les deux extrémités sont sur l'étage actif (rendu + hit).
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

    // Pins précalculés (les `let` ne se déclarent pas dans le corps rsx) :
    // (scène, surlignée, départ, fill, stroke effectif).
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
                    // Fond : désélection + pan (jamais en mode lien).
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
                    // Échap : désélection (l'inspecteur se replie avec).
                    if event.key() == Key::Escape {
                        cx.selected.set(None);
                    }
                },
                g { transform: "translate({pan.0} {pan.1}) scale({zoom})",
                    // Plan (image étirée aux dimensions déclarées — repli
                    // 2000×1000 si inconnues ; doc studio.md « reste ouvert »).
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
                    // Liens (ligne + pastille cible ; stair = trait mixte).
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
                        // Hit transparent plus large pour la sélection du lien.
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
                    // Lien en attente (mode lien, source posée) vers le centre
                    // du plan — purement indicatif.
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
                    // Scènes : pin cliquable + étiquette (composant dédié —
                    // école CanvasNode : les gestes vivent dans le composant).
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
            // Étage sans plan : bandeau d'invite (scènes posables après
            // import — l'éditeur exige un plan pour positionner).
            if active_floor.as_ref().and_then(|f| f.plan.as_ref()).is_none() {
                div { class: "absolute inset-0 flex items-center justify-center pointer-events-none",
                    p { class: "text-sm text-gray-400 bg-white/80 rounded-lg px-4 py-2",
                        {t!("studio-canvas-no-plan")}
                    }
                }
            }
        }
    }
}

/// Pin d'une scène : cercle cliquable + étiquette + tag « départ ».
/// Composant dédié (école `CanvasNode`) : les props sont possédés, les
/// gestes capturent la scène par valeur sans conflit de borrow.
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
    // Capturé par la closure de geste (le composant capture `scene` une
    // seule fois — école CanvasNode : un clone local nommé par closure).
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
                fill: "#374151",
                "pointer-events": "none",
                {if scene.label.is_empty() { scene.id.clone() } else { scene.label.clone() }}
            }
            if is_start {
                text {
                    x: "{scene.x}",
                    y: "{scene.y - geometry::SCENE_RADIUS - 6.0}",
                    "text-anchor": "middle",
                    "font-size": "12",
                    fill: "#10b981",
                    "pointer-events": "none",
                    {t!("studio-scene-start-tag")}
                }
            }
        }
    }
}

/// pointerdown sur une scène : mode lien → pose la source/crée le lien ;
/// sinon début du drag (grab = décalage curseur↔pin, mesuré ici).
fn scene_pointer_down(scene: &pnex_core::TourScene, mut cx: TourEditorCx, client: (f64, f64)) {
    if cx.link_mode.cloned() {
        let target = scene.id.clone();
        if let Some(from) = cx.link_from.cloned() {
            if from != target {
                // Paire aller-retour + angles auto (yaw = direction plan,
                // pitch = ±30° si étages différents) — cf. state::connect_scenes.
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
        return; // natif : sélection sans drag
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
            // Pas de snap grille : l'unité est le px natif du plan (libre).
            let pos = (point.0 - grab.0, point.1 - grab.1);
            cx.update_doc(move |doc| state::move_scene(doc, &id, pos));
        }
    }
}

fn canvas_pointer_up(mut cx: TourEditorCx) {
    cx.interaction.set(Interaction::Idle);
}

/// Molette : zoom vers le curseur, borné (école flow_editor).
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
