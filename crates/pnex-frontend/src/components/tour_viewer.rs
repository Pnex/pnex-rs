//! Read-only tour viewer: a single scene mounted at a time
//! (S13), navigation by hotspots (`take_nav` polling) + floor
//! selector + mini-plan. Used by the editor preview (bytes via
//! authenticated paths) and by the public `/share/:token` page (bytes
//! via the public endpoint, no auth).

use dioxus::prelude::*;
use dioxus_i18n::t;
use pnex_core::{ResolvedAnnotationItem, TourDoc};
use std::collections::HashMap;

use crate::components::annotation_editor::popover::AnnotationPopover;
use crate::components::surface::annotation::{
    effective_mode, is_surface_item, AnnotMode, AnnotModeToggle, AnnotationCardsOverlay,
};
use crate::tour_viewer::{AnnotMarkerView, TourHotspotView, TourNav, TourSceneView};

/// Source of the asset bytes (path builder).
#[derive(Clone, PartialEq)]
pub enum ViewerSource {
    /// In-app: `/api/v1/media/{id}/content` (current version,
    /// authenticated — same path as the Media page).
    Auth,
    /// Public : `/api/v1/public/tours/{token}/assets/{id}?v=n`.
    Public { token: String },
}

/// Info per referenced asset (version + mime known on the public side;
/// `None` in-app — the current content is served).
#[derive(Clone, Default, PartialEq)]
pub struct AssetInfo {
    pub version: Option<i64>,
    pub content_type: Option<String>,
}

/// **Default** host div of the pannellum viewer — a given instance must
/// have a unique host (JS bridge by DOM id): new callers
/// pass their own `host_id` (e.g. POI preview).
pub const HOST_ID: &str = "pnex-tour-viewer-host";

/// Floor button: (floor, active, first scene of the floor — target).
type FloorButton = (pnex_core::TourFloor, bool, Option<(String, String)>);

#[component]
pub fn TourViewer(
    doc: TourDoc,
    assets: HashMap<String, AssetInfo>,
    source: ViewerSource,
    /// Dragging a hotspot in preview: (link_id, yaw, pitch) — the editor
    /// patches its doc; the share page passes a no-op.
    on_hotspot_move: Callback<(String, f64, f64)>,
    /// DOM id of the pannellum host — unique per instance (JS bridge by id).
    host_id: String,
    /// Compact mode (POI preview): fills the parent (`flex-1 min-h-0`,
    /// full-frame panel over the map) instead of `h-[70vh]` — the pannellum
    /// host stays absolutely positioned (height:100% trap unchanged).
    compact: bool,
    /// Annotations overlay (D58): read model fetch + markers. The
    /// public share page passes `false` (annotations may reveal
    /// device data).
    annotations_enabled: bool,
    /// Current scene change (id) — the annotation editor follows the
    /// displayed media; other callers pass a no-op.
    on_scene_change: Callback<String>,
    /// Side panel (floors/mini-plan/scenes) — hidden on the annotations
    /// page for a single media (no tour chrome to show).
    show_side_panel: bool,
    /// Read context of the annotations (D147): `Some(tour id)` = this tour's
    /// sets only; `None` = the standalone media's sets (single-media doc).
    #[props(default)]
    annotation_tour: Option<String>,
    /// Gated marker (arrow) drag: true only in the tour editor preview —
    /// read-only viewers (map POI preview, share page, annotation page)
    /// keep markers fixed. Pushed to the JS host guard at event time.
    editable: bool,
) -> Element {
    // Current scene: explicit start, else the first scene.
    let mut current = use_signal(|| {
        doc.start_scene
            .clone()
            .or_else(|| doc.scenes.first().map(|s| s.id.clone()))
            .unwrap_or_default()
    });
    let mut active_floor = use_signal(|| {
        doc.scenes
            .iter()
            .find(|s| s.id == current.cloned())
            .map(|s| s.floor_id.clone())
            .or_else(|| doc.floors.first().map(|f| f.id.clone()))
            .unwrap_or_default()
    });
    let mut nav_seq = use_signal(|| 0u64);
    let mut move_seq = use_signal(|| 0u64);
    // Poll guard (a single loop per viewer) — the write happens in
    // an effect, never during render.
    let mut poll_started = use_signal(|| false);
    let mut mounted = use_signal(|| false);
    // In-progress operation guard (mount) + last applied (scene, url) —
    // serialisation of viewer operations, see the comment in the effect.
    let mut mounting = use_signal(|| false);
    let mut applied = use_signal(|| None::<(String, String)>);
    let mut load_failed = use_signal(|| false);

    // ─── Annotations (D58): toggle, read model items, popover ───
    // "Annotations" toggle (on by default); the viewer's poll loop
    // is extended to `take_annot_click` (monotonic seq); applying the
    // markers is an **effect separate** from the mount (the mount's dedup key
    // stays (scene, url) — NEVER the annotations, otherwise a remount on every
    // edit).
    // Viewer's display choice (None = automatic: cards when the media
    // carries control / reading items, dots otherwise — D147).
    let mut annot_mode = use_signal(|| None::<AnnotMode>);
    let mut annot_has_cards = use_signal(|| false);
    let mut annot_seq = use_signal(|| 0u64);
    let mut annot_selected = use_signal(|| None::<ResolvedAnnotationItem>);
    // Last applied marker batch (scene, url, json) — dedup of
    // double effect invocations; the scene + url come from `applied`
    // (the mount/switch must be FINISHED before setting the markers).
    let mut applied_annot = use_signal(|| None::<(String, String, String)>);

    // Blob/data-URI URL of the current scene's panorama (fetched on the Rust side) —
    // the resource re-reads a **Memo** (asset of the current scene): a capture
    // by value never restarts the resource (dioxus trap #2784, see
    // app.rs) → the panorama never changed scene.
    let doc_for_memo = doc.clone();
    let scene_asset = use_memo(move || {
        let doc = doc_for_memo.clone();
        let current = current.cloned();
        doc.scenes
            .iter()
            .find(|s| s.id == current)
            .map(|s| s.media_asset_id.clone())
    });
    let source_for_url = source.clone();
    let assets_for_url = assets.clone();
    let url = use_resource(move || {
        let asset_id = scene_asset.read().clone();
        let source = source_for_url.clone();
        let assets = assets_for_url.clone();
        async move {
            let asset_id = asset_id?;
            let info = assets.get(&asset_id).cloned().unwrap_or_default();
            let path = match &source {
                ViewerSource::Auth => crate::api::media::content_path(&asset_id),
                ViewerSource::Public { token } => {
                    crate::api::tours::public_asset_path(token, &asset_id, info.version)
                }
            };
            crate::util::media_blob_url(&path, info.content_type.as_deref()).await
        }
    });

    // Annotations read model of the displayed asset — the resource re-reads the
    // `scene_asset` Memo INSIDE the closure (dioxus trap #2784): follows
    // scene changes. Errors swallowed → empty overlay (S7).
    // Disabled by the prop (share page: annotations_enabled = false).
    let annotations = use_resource(move || {
        let asset_id = scene_asset.read().clone();
        let enabled = annotations_enabled;
        let tour = annotation_tour.clone();
        async move {
            if !enabled {
                return None;
            }
            let asset_id = asset_id?;
            crate::api::annotation_layers::media_annotations(&asset_id, tour.as_deref())
                .await
                .ok()
        }
    });

    // Plan of the active floor for the mini-map (authenticated blob, same chain
    // as the panorama) — Memo read INSIDE the resource closure (dioxus
    // trap #2784): follows floor changes.
    let doc_for_plan = doc.clone();
    let active_plan = use_memo(move || {
        let doc = doc_for_plan.clone();
        let floor_id = active_floor.cloned();
        doc.floors
            .iter()
            .find(|f| f.id == floor_id)
            .and_then(|f| f.plan.as_ref())
            .map(|p| p.media_asset_id.clone())
    });
    let plan_url = use_resource(move || {
        let asset_id = active_plan.read().clone();
        let source = source.clone();
        let assets = assets.clone();
        async move {
            let asset_id = asset_id?;
            let info = assets.get(&asset_id).cloned().unwrap_or_default();
            let path = match &source {
                ViewerSource::Auth => crate::api::media::content_path(&asset_id),
                ViewerSource::Public { token } => {
                    crate::api::tours::public_asset_path(token, &asset_id, info.version)
                }
            };
            crate::util::media_blob_url(&path, info.content_type.as_deref()).await
        }
    });

    // Mount / switch when the URL is ready (a single texture in memory).
    // The doc is cloned per invocation (FnMut closure — never a move of the
    // captured prop).
    let doc_for_effect = doc.clone();
    let host_for_effect = host_id.clone();
    use_effect(move || {
        let ready = url.value().read().clone().flatten();
        let Some(url_value) = ready else {
            return;
        };
        let doc = doc_for_effect.clone();
        let Some(scene) = doc
            .scenes
            .iter()
            .find(|s| s.id == current.cloned())
            .cloned()
        else {
            return;
        };
        // Hotspots: outgoing links of the scene whose target exists
        // (dead reference → hotspot simply missing, never a panic).
        let hotspots: Vec<TourHotspotView> = doc
            .links
            .iter()
            .filter(|link| link.from == scene.id)
            .filter_map(|link| {
                let target = doc.scenes.iter().find(|s| s.id == link.to)?;
                Some(TourHotspotView {
                    yaw: link.yaw,
                    pitch: link.pitch,
                    target_scene: link.to.clone(),
                    target_floor: target.floor_id.clone(),
                    label: link.label.clone().unwrap_or_else(|| target.label.clone()),
                    kind: link.kind.clone(),
                    link_id: link.id.clone(),
                })
            })
            .collect();
        let view = TourSceneView {
            scene_id: scene.id.clone(),
            floor_id: scene.floor_id.clone(),
            url: url_value.clone(),
            yaw: scene.initial_yaw,
            pitch: scene.initial_pitch,
            hfov: scene.initial_fov,
            hotspots,
        };
        // Dedup: the same (scene, url) already applied does not re-trigger
        // an operation (dioxus's double effect invocation + re-renders
        // without a real change would otherwise chain destroy/re-create
        // for nothing — and an operation while pannellum is still loading its
        // texture leaves the viewer empty).
        let applied_key = (scene.id.clone(), url_value.clone());
        if applied.cloned().as_ref() == Some(&applied_key) {
            return;
        }
        if !mounted() {
            // One mount at a time (`mounting` guard); `mounted` is only
            // set after success — a scene change during the mount
            // retries is re-applied by the effect when `mounting`
            // drops (the effect reads both signals), no concurrent switch.
            if mounting() {
                return;
            }
            mounting.set(true);
            let host = host_for_effect.clone();
            let view = view.clone();
            spawn(async move {
                let ok = crate::tour_viewer::mount(&host, &view).await;
                mounting.set(false);
                load_failed.set(!ok);
                if ok {
                    mounted.set(true);
                    applied.set(Some(applied_key));
                }
            });
        } else {
            // Switch with retry (like the mount); total failure = badge —
            // no more empty viewer without feedback, and a following nav retries
            // (load_failed = !ok clears on the next success).
            let host = host_for_effect.clone();
            let view = view.clone();
            spawn(async move {
                let ok = crate::tour_viewer::switch_scene(&host, &view).await;
                load_failed.set(!ok);
                if ok {
                    applied.set(Some(applied_key));
                }
            });
        }
    });

    // Polling of viewer events (hotspot nav + arrow drag): a
    // single loop, started once per effect — the old pattern
    // "signal written during render to restart itself" died in dioxus 0.7
    // (write during render dropped → loop killed on the first tick, observed
    // 2026-09-12: hotspot clicks no longer did anything). The task
    // is scoped to the component: killed when the modal closes.
    use_effect(move || {
        if poll_started() {
            return;
        }
        poll_started.set(true);
        spawn(async move {
            loop {
                crate::util::sleep(std::time::Duration::from_millis(250)).await;
                if let Some(TourNav {
                    seq,
                    scene_id,
                    floor_id,
                }) = crate::tour_viewer::take_nav(nav_seq.cloned()).await
                {
                    nav_seq.set(seq);
                    // Scene change: the popover of an item of the old
                    // scene no longer makes sense.
                    annot_selected.set(None);
                    current.set(scene_id);
                    active_floor.set(floor_id);
                }
                // Arrow drag: pass (link_id, yaw, pitch) up to the parent.
                if let Some(mv) = crate::tour_viewer::take_hotspot_move(move_seq.cloned()).await {
                    move_seq.set(mv.seq);
                    on_hotspot_move.call((mv.link_id, mv.yaw, mv.pitch));
                }
                // Click on an annotation marker (D58): open the popover
                // with the resolved item (lookup by id in the current batch).
                if annotations_enabled {
                    if let Some(click) =
                        crate::tour_viewer::take_annot_click(annot_seq.cloned()).await
                    {
                        annot_seq.set(click.seq);
                        let found = annotations
                            .value()
                            .read()
                            .as_ref()
                            .and_then(|o| o.as_ref())
                            .and_then(|a| {
                                a.items.iter().find(|it| it.id == click.item_id).cloned()
                            });
                        // Cards mode: a control / reading item already shows
                        // its card on the marker — no popover on top of it.
                        let cards_mode =
                            effective_mode(annot_mode.cloned(), annot_has_cards.cloned())
                                == AnnotMode::Cards;
                        let found = found.filter(|it| !(cards_mode && is_surface_item(it)));
                        annot_selected.set(found);
                    }
                }
            }
        });
    });

    // Viewer unmount (MediaPreview use_drop pattern).
    let host_for_drop = host_id.clone();
    use_drop(move || {
        crate::tour_viewer::unmount(&host_for_drop);
    });

    // Arrow-drag gate: push the editable flag to the JS host guard (read at
    // event time, so the flag can land before or after the first mount —
    // bridge retries while the bundle/host settle).
    let host_for_editable = host_id.clone();
    use_effect(move || {
        let host = host_for_editable.clone();
        spawn(async move {
            crate::tour_viewer::set_tour_editable(&host, editable).await;
        });
    });

    // Notify the current scene (INITIAL scene included — without it
    // the annotation editor ignores the starting media and placement
    // fails silently until the first navigation, observed
    // 2026-09-16). Single source: replaces the scattered calls in
    // the nav/button handlers.
    use_effect(move || {
        let scene_id = current.cloned();
        if scene_id.is_empty() {
            return;
        }
        on_scene_change.call(scene_id);
    });

    // Applying the markers (D58): effect **separate** from the mount — it
    // acts only after the mount/switch has finished (`applied` == current key),
    // dedup by (scene, url, json). `set_annotations` = remove/add of
    // hotspots post-mount, never a remount.
    let annot_enabled_for_effect = annotations_enabled;
    let host_for_annot = host_id.clone();
    use_effect(move || {
        if !annot_enabled_for_effect {
            return;
        }
        let Some((scene_id, url_key)) = applied.cloned() else {
            return;
        };
        let items = annotations
            .value()
            .read()
            .as_ref()
            .and_then(|o| o.as_ref())
            .map(|a| a.items.clone())
            .unwrap_or_default();
        let mode = effective_mode(annot_mode(), annot_has_cards());
        let markers: Vec<AnnotMarkerView> = if mode != AnnotMode::Hidden {
            items
                .iter()
                .filter_map(|it| match &it.geometry {
                    pnex_core::AnnotationGeometry::Equirect { yaw, pitch } => {
                        Some(AnnotMarkerView {
                            id: it.id.clone(),
                            yaw: *yaw,
                            pitch: *pitch,
                            kind: it.kind.clone(),
                            label: it.label.clone(),
                        })
                    }
                    _ => None,
                })
                .collect()
        } else {
            Vec::new()
        };
        let json = serde_json::to_string(&markers).unwrap_or_default();
        let key = (scene_id, url_key, json.clone());
        if applied_annot.cloned().as_ref() == Some(&key) {
            return;
        }
        applied_annot.set(Some(key));
        let host = host_for_annot.clone();
        spawn(async move {
            crate::tour_viewer::set_annotations(&host, &markers, false).await;
        });
    });

    // Has the shown media control / reading items? (drives the automatic
    // mode — written from an effect, never during render).
    use_effect(move || {
        let has = annotations
            .value()
            .read()
            .as_ref()
            .and_then(|o| o.as_ref())
            .is_some_and(|a| a.items.iter().any(is_surface_item));
        if annot_has_cards() != has {
            annot_has_cards.set(has);
        }
    });

    // Cards follow their pannellum hotspots (glue RAF, cancelled on unmount
    // or when the cards are hidden).
    let host_for_cards = host_id.clone();
    use_effect(move || {
        let on = annotations_enabled
            && annot_has_cards()
            && effective_mode(annot_mode(), annot_has_cards()) == AnnotMode::Cards;
        let host = host_for_cards.clone();
        let overlay = format!("{host}-cards");
        spawn(async move {
            crate::tour_viewer::follow_cards(&host, &overlay, on).await;
        });
    });

    // ─── Precomputations (no `let` in the rsx body) ───
    let current_id = current.cloned();
    let active_floor_id = active_floor.cloned();
    let floor_scenes: Vec<pnex_core::TourScene> = doc
        .scenes
        .iter()
        .filter(|s| s.floor_id == active_floor_id)
        .cloned()
        .collect();
    let floors: Vec<pnex_core::TourFloor> = doc.floors.clone();
    let (plan_w, plan_h) = match doc
        .floors
        .iter()
        .find(|f| f.id == active_floor_id)
        .and_then(|f| f.plan.as_ref())
    {
        Some(plan) => (plan.width.unwrap_or(2000.0), plan.height.unwrap_or(1000.0)),
        None => (2000.0, 1000.0),
    };
    // (floor, active, first scene of the floor — button target).
    let floor_buttons: Vec<FloorButton> = floors
        .iter()
        .map(|floor| {
            let first = doc
                .scenes
                .iter()
                .find(|s| s.floor_id == floor.id)
                .map(|s| (s.id.clone(), floor.id.clone()));
            (floor.clone(), *floor.id == active_floor_id, first)
        })
        .collect();
    // Mini-plan: (id, cx, cy, current) in a 200×100 viewBox.
    let mini_dots: Vec<(String, f64, f64, bool)> = floor_scenes
        .iter()
        .map(|s| {
            (
                s.id.clone(),
                s.x / plan_w * 200.0,
                s.y / plan_h * 100.0,
                s.id == current_id,
            )
        })
        .collect();
    // (scene, current) for the clickable list.
    let scene_buttons: Vec<(pnex_core::TourScene, bool)> = floor_scenes
        .iter()
        .map(|s| (s.clone(), s.id == current_id))
        .collect();
    // Plan image behind the dots (None until the blob is ready).
    let mini_plan_url = plan_url.value().read().clone().flatten();
    // Annotations: toggle rendered if ≥ 1 item on the displayed media; item
    // selected for the popover.
    let has_items = annotations
        .value()
        .read()
        .as_ref()
        .and_then(|o| o.as_ref())
        .is_some_and(|a| !a.items.is_empty());
    let annot_selected_now = annot_selected.cloned();
    // Control / reading items of the shown media: cards anchored next to
    // their markers (D147), when the viewer shows cards.
    let surface_items: Vec<ResolvedAnnotationItem> = annotations
        .value()
        .read()
        .as_ref()
        .and_then(|o| o.as_ref())
        .map(|a| {
            a.items
                .iter()
                .filter(|i| is_surface_item(i))
                .cloned()
                .collect()
        })
        .unwrap_or_default();
    let surface_key = surface_items
        .iter()
        .map(|i| i.id.as_str())
        .collect::<Vec<_>>()
        .join(",");
    let mode_now = effective_mode(annot_mode(), !surface_items.is_empty());
    let show_cards =
        annotations_enabled && mode_now == AnnotMode::Cards && !surface_items.is_empty();
    let overlay_id = format!("{host_id}-cards");

    rsx! {
        div { class: if compact { "flex w-full gap-3 flex-1 min-h-0" } else { "flex w-full gap-3 h-[70vh]" },
            // Side panel: floors + mini-plan + scenes (driven by
            // show_side_panel, including in compact mode).
            if show_side_panel {
                div { class: "w-60 shrink-0 flex flex-col gap-2 overflow-y-auto",
                    div {
                        span { class: "text-xs font-semibold text-gray-500",
                            {t!("tour-viewer-floors")}
                        }
                        div { class: "flex flex-col gap-1 mt-1",
                            for (floor, is_active, target) in floor_buttons.clone() {
                                button {
                                    key: "{floor.id}",
                                    class: if target.is_none() { "w-full text-left px-2 py-1.5 text-sm rounded-lg bg-gray-50 text-gray-500 border border-transparent cursor-not-allowed" } else if is_active { "w-full text-left px-2 py-1.5 text-sm rounded-lg bg-signal-50 text-signal-700 border border-signal-200" } else { "w-full text-left px-2 py-1.5 text-sm rounded-lg hover:bg-gray-50 text-gray-700 border border-transparent" },
                                    // A floor without scenes has nothing to show: a silent
                                    // no-op = confusion (observed 2026-09-16) —
                                    // visually disabled.
                                    disabled: target.is_none(),
                                    onclick: move |_| {
                                        if let Some((scene_id, floor_id)) = target.clone() {
                                            current.set(scene_id);
                                            active_floor.set(floor_id);
                                        }
                                    },
                                    {if floor.name.is_empty() { floor.id.clone() } else { floor.name.clone() }}
                                }
                            }
                        }
                    }
                    // Mini-plan: plan image + scene positions (the dots
                    // share the viewBox's x/plan_w*200, y/plan_h*100 mapping).
                    if !mini_dots.is_empty() {
                        div { class: "bg-white border border-gray-200 rounded-lg p-1",
                            svg { view_box: "0 0 200 100", class: "w-full",
                                if let Some(url) = mini_plan_url {
                                    image {
                                        href: "{url}",
                                        x: "0",
                                        y: "0",
                                        width: "200",
                                        height: "100",
                                        preserve_aspect_ratio: "none",
                                        "pointer-events": "none",
                                    }
                                }
                                for (id, dot_x, dot_y, is_current) in mini_dots.clone() {
                                    circle {
                                        key: "mini-{id}",
                                        cx: "{dot_x}",
                                        cy: "{dot_y}",
                                        r: if is_current { "5" } else { "3" },
                                        fill: if is_current { "#2563eb" } else { "#94a3b8" },
                                        "pointer-events": "none",
                                    }
                                }
                            }
                        }
                    }
                    // List of the floor's clickable scenes.
                    ul { class: "divide-y divide-gray-100",
                        for (scene, is_current) in scene_buttons.clone() {
                            li { key: "list-{scene.id}",
                                button {
                                    class: if is_current { "w-full text-left px-2 py-1.5 text-sm bg-signal-50 text-signal-700 rounded-lg" } else { "w-full text-left px-2 py-1.5 text-sm hover:bg-gray-50 text-gray-700 rounded-lg" },
                                    onclick: move |_| {
                                        current.set(scene.id.clone());
                                        active_floor.set(scene.floor_id.clone());
                                    },
                                    {if scene.label.is_empty() { scene.id.clone() } else { scene.label.clone() }}
                                }
                            }
                        }
                    }
                }
            }

            // Viewer host: relative wrapper + absolutely positioned host
            // (pannellum trap — MediaPreview school: .pnlm-container sets
            // height:100% after Tailwind; never size the host with a class).
            div { class: "flex-1 relative min-w-0",
                // Display switch (D147): hidden / dots / cards — shown when
                // the media carries items; closes the popover on change.
                if has_items && annotations_enabled {
                    AnnotModeToggle {
                        mode: mode_now,
                        has_cards: !surface_items.is_empty(),
                        on_change: move |m| {
                            annot_mode.set(Some(m));
                            annot_selected.set(None);
                        },
                    }
                }
                if load_failed() {
                    div {
                        class: "flex items-center justify-center bg-gray-100 rounded-lg",
                        style: "height: 320px;",
                        p { class: "text-sm text-gray-500", {t!("tour-viewer-unavailable")} }
                    }
                }
                div {
                    id: "{host_id}",
                    style: "position: absolute; inset: 0; height: 100%;",
                }
                if show_cards {
                    // Keyed on the item set: the cards' live poll captures it.
                    div { key: "{surface_key}",
                        AnnotationCardsOverlay {
                            items: surface_items.clone(),
                            overlay_id: overlay_id.clone(),
                        }
                    }
                }
                // Live popover (D58): clicked item, outside the canvas (dioxus
                // panel), never blocking.
                if let Some(item) = annot_selected_now {
                    div { key: "annot-pop-{item.id}",
                        AnnotationPopover {
                            item,
                            on_close: move |_| annot_selected.set(None),
                        }
                    }
                }
            }
        }
    }
}
