//! Read-only view of one media with its PUBLISHED annotations (D58, D147)
//! — the single viewer behind every "look at this media" surface (map POI
//! preview, media page, annotation page preview). Panorama → `TourViewer`
//! on a one-scene synthetic doc; photo/floorplan → image + %-positioned
//! markers; splat → gsplat viewer + projected 3D markers. Each shows the
//! hidden / dots / cards switch: control and reading items become small
//! dashboard cards anchored next to their marker.

use dioxus::prelude::*;
use pnex_core::{AnnotationGeometry, ResolvedAnnotationItem, TourDoc};

use crate::api;
use crate::components::annotation_editor::popover::{annot_dot_color, AnnotationPopover};
use crate::components::surface::annotation::{
    effective_mode, is_surface_item, AnnotMode, AnnotModeToggle, AnnotationCardsOverlay,
};
use crate::components::tour_viewer::{TourViewer, ViewerSource};
use crate::tour_viewer::SplatMarkerView;
use crate::util::media_blob_url;

/// One-scene tour doc: lets the tour viewer act as the pannellum viewer of
/// a single panorama, without any tour chrome.
pub fn single_media_doc(media_asset_id: &str) -> TourDoc {
    let mut doc = TourDoc::default();
    doc.scenes.push(pnex_core::TourScene {
        id: "annot-scene".into(),
        floor_id: "f".into(),
        label: String::new(),
        media_asset_id: media_asset_id.to_string(),
        x: 0.0,
        y: 0.0,
        initial_yaw: 0.0,
        initial_pitch: 0.0,
        initial_fov: 100.0,
    });
    doc.start_scene = Some("annot-scene".into());
    doc
}

/// Media + published annotations, read-only. `kind` is the media kind
/// string (`panorama`, `photo`, `floorplan`, `splat`); `compact` fills the
/// parent (`flex-1 min-h-0`) instead of a fixed 70vh height.
#[component]
pub fn AnnotatedMediaView(
    asset_id: String,
    kind: String,
    host_id: String,
    compact: bool,
) -> Element {
    match kind.as_str() {
        "panorama" => rsx! {
            TourViewer {
                key: "annotated-{asset_id}",
                doc: single_media_doc(&asset_id),
                assets: Default::default(),
                source: ViewerSource::Auth,
                on_hotspot_move: move |_: (String, f64, f64)| {},
                on_scene_change: move |_: String| {},
                host_id,
                compact,
                annotations_enabled: true,
                show_side_panel: false,
                editable: false,
            }
        },
        "splat" => rsx! {
            SplatAnnotatedView {
                key: "splat-annotated-{asset_id}",
                asset_id,
                host_id,
                compact,
            }
        },
        _ => rsx! {
            FlatAnnotatedImage { key: "flat-annotated-{asset_id}", asset_id, compact }
        },
    }
}

/// Published annotations of a media (read model, errors → none).
fn use_media_annotations(asset_id: String) -> Resource<Option<pnex_core::MediaAnnotations>> {
    use_resource(move || {
        let id = asset_id.clone();
        async move {
            api::annotation_layers::media_annotations(&id, None)
                .await
                .ok()
        }
    })
}

/// Items of the read model (empty while loading).
fn items_of(res: &Resource<Option<pnex_core::MediaAnnotations>>) -> Vec<ResolvedAnnotationItem> {
    res.value()
        .read()
        .as_ref()
        .and_then(|o| o.as_ref())
        .map(|a| a.items.clone())
        .unwrap_or_default()
}

/// Comma-joined ids: key of the card overlay (its live poll captures the
/// item set at mount).
fn ids_key(items: &[ResolvedAnnotationItem]) -> String {
    items
        .iter()
        .map(|i| i.id.as_str())
        .collect::<Vec<_>>()
        .join(",")
}

/// Flat image with its published annotation markers (fractions of the
/// drawn image box — the wrapper shrinks to the image, so % stay aligned).
#[component]
fn FlatAnnotatedImage(asset_id: String, compact: bool) -> Element {
    let mut blob_url = use_signal(|| None::<String>);
    let mut selected = use_signal(|| None::<ResolvedAnnotationItem>);
    let mut mode = use_signal(|| None::<AnnotMode>);
    let id_for_blob = asset_id.clone();
    use_effect(move || {
        let id = id_for_blob.clone();
        spawn(async move {
            let url = media_blob_url(&api::media::content_path(&id), None).await;
            blob_url.set(url);
        });
    });
    let annotations = use_media_annotations(asset_id.clone());
    let items: Vec<ResolvedAnnotationItem> = items_of(&annotations)
        .into_iter()
        .filter(|it| matches!(it.geometry, AnnotationGeometry::Flat { .. }))
        .collect();
    let cards: Vec<ResolvedAnnotationItem> = items
        .iter()
        .filter(|i| is_surface_item(i))
        .cloned()
        .collect();
    let mode_now = effective_mode(mode(), !cards.is_empty());
    // (id, x, y, kind, item) — no `let` inside the rsx body.
    let markers: Vec<(String, f64, f64, String, ResolvedAnnotationItem)> =
        if mode_now == AnnotMode::Hidden {
            Vec::new()
        } else {
            items
                .iter()
                .filter_map(|it| match &it.geometry {
                    AnnotationGeometry::Flat { x, y } => {
                        Some((it.id.clone(), *x, *y, it.kind.clone(), it.clone()))
                    }
                    _ => None,
                })
                .collect()
        };
    let show_cards = mode_now == AnnotMode::Cards && !cards.is_empty();
    let cards_key = ids_key(&cards);
    let overlay_id = format!("annot-flat-cards-{}", asset_id.replace('-', ""));
    let frame_class = if compact {
        "relative flex-1 min-h-0 bg-gray-900 flex items-center justify-center p-3"
    } else {
        "relative bg-gray-900 rounded-lg flex items-center justify-center h-[70vh] p-3"
    };

    rsx! {
        div { class: "{frame_class}",
            if !items.is_empty() {
                AnnotModeToggle {
                    mode: mode_now,
                    has_cards: !cards.is_empty(),
                    on_change: move |m| {
                        mode.set(Some(m));
                        selected.set(None);
                    },
                }
            }
            match blob_url.cloned() {
                Some(url) => rsx! {
                    div { class: "relative inline-flex",
                        img {
                            src: "{url}",
                            class: "max-h-[calc(70vh-24px)] max-w-full object-contain select-none",
                            draggable: "false",
                        }
                        for (m_id, m_x, m_y, m_kind, m_item) in markers {
                            button {
                                key: "annot-{m_id}",
                                class: "absolute z-10 h-5 w-5 -translate-x-1/2 -translate-y-1/2 rounded-full border-2 border-white shadow-md hover:scale-110 transition-transform {annot_dot_color(&m_kind)}",
                                style: "left: calc({m_x} * 100%); top: calc({m_y} * 100%);",
                                title: "{m_item.label}",
                                // Cards mode: the card already sits on the marker.
                                onclick: move |_| {
                                    if !(mode_now == AnnotMode::Cards && is_surface_item(&m_item)) {
                                        selected.set(Some(m_item.clone()));
                                    }
                                },
                            }
                        }
                        if show_cards {
                            div { key: "{cards_key}",
                                AnnotationCardsOverlay {
                                    items: cards.clone(),
                                    overlay_id: overlay_id.clone(),
                                    flat: true,
                                }
                            }
                        }
                    }
                },
                None => rsx! {
                    span { class: "animate-spin rounded-full h-8 w-8 border-b-2 border-white" }
                },
            }
            if let Some(item) = selected.cloned() {
                div { key: "annot-pop-{item.id}",
                    AnnotationPopover { item, on_close: move |_| selected.set(None) }
                }
            }
        }
    }
}

/// Gaussian splat mounted in `host_id` (blob of the current version) —
/// shared by the read-only view and the annotation editor. The host is
/// absolutely positioned inside the caller's `relative` box.
#[component]
pub fn SplatHost(asset_id: String, host_id: String) -> Element {
    let mut failed = use_signal(|| false);
    let host_for_mount = host_id.clone();
    let host_for_drop = host_id.clone();
    use_effect(move || {
        let id = asset_id.clone();
        let host = host_for_mount.clone();
        spawn(async move {
            let ok = match media_blob_url(&api::media::content_path(&id), None).await {
                Some(url) => crate::media_viewer::mount("splat", &host, &url).await,
                None => false,
            };
            failed.set(!ok);
        });
    });
    use_drop(move || {
        crate::media_viewer::unmount(&host_for_drop);
    });
    rsx! {
        div { id: "{host_id}", style: "position: absolute; inset: 0;" }
        if failed() {
            div { class: "absolute inset-0 flex items-center justify-center pointer-events-none",
                span { class: "text-sm text-gray-400", {dioxus_i18n::t!("poi-media-unavailable")} }
            }
        }
    }
}

/// Splat + its published annotations: projected markers, popover on
/// click, cards that follow the orbit.
#[component]
fn SplatAnnotatedView(asset_id: String, host_id: String, compact: bool) -> Element {
    let mut selected = use_signal(|| None::<ResolvedAnnotationItem>);
    let mut mode = use_signal(|| None::<AnnotMode>);
    let mut applied = use_signal(|| None::<String>);
    let mut poll_started = use_signal(|| false);
    let mut click_seq = use_signal(|| 0u64);
    let annotations = use_media_annotations(asset_id.clone());

    // Markers pushed to the glue (dedup by JSON; retry until the viewer is
    // mounted) — hidden mode clears them.
    let host_markers = host_id.clone();
    use_effect(move || {
        let items = items_of(&annotations);
        let has_cards = items.iter().any(is_surface_item);
        let visible = effective_mode(mode(), has_cards) != AnnotMode::Hidden;
        let markers: Vec<SplatMarkerView> = if visible {
            items
                .iter()
                .filter_map(|it| match it.geometry {
                    AnnotationGeometry::Splat { x, y, z } => Some(SplatMarkerView {
                        id: it.id.clone(),
                        x,
                        y,
                        z,
                        kind: it.kind.clone(),
                        label: it.label.clone(),
                    }),
                    _ => None,
                })
                .collect()
        } else {
            Vec::new()
        };
        let json = serde_json::to_string(&markers).unwrap_or_default();
        if applied.cloned().as_deref() == Some(json.as_str()) {
            return;
        }
        applied.set(Some(json));
        let host = host_markers.clone();
        spawn(async move {
            crate::tour_viewer::set_splat_annotations(&host, &markers, false).await;
        });
    });

    // Marker click → popover (single poll loop per view).
    use_effect(move || {
        if poll_started() {
            return;
        }
        poll_started.set(true);
        spawn(async move {
            loop {
                crate::util::sleep(std::time::Duration::from_millis(250)).await;
                if let Some(c) = crate::tour_viewer::take_annot_click(click_seq.cloned()).await {
                    click_seq.set(c.seq);
                    let items = items_of(&annotations);
                    let has_cards = items.iter().any(is_surface_item);
                    // Cards mode: the card is already on the marker.
                    let cards_mode = effective_mode(mode.cloned(), has_cards) == AnnotMode::Cards;
                    let found = items
                        .into_iter()
                        .find(|it| it.id == c.item_id)
                        .filter(|it| !(cards_mode && is_surface_item(it)));
                    selected.set(found);
                }
            }
        });
    });

    let items = items_of(&annotations);
    let cards: Vec<ResolvedAnnotationItem> = items
        .iter()
        .filter(|i| is_surface_item(i))
        .cloned()
        .collect();
    let mode_now = effective_mode(mode(), !cards.is_empty());
    let show_cards = mode_now == AnnotMode::Cards && !cards.is_empty();
    let cards_key = ids_key(&cards);
    let overlay_id = format!("{host_id}-cards");

    // The splat render loop places the cards (projected markers).
    let host_cards = host_id.clone();
    let overlay_for_follow = overlay_id.clone();
    use_effect(move || {
        let has_cards = items_of(&annotations).iter().any(is_surface_item);
        let on = has_cards && effective_mode(mode(), has_cards) == AnnotMode::Cards;
        let host = host_cards.clone();
        let overlay = overlay_for_follow.clone();
        spawn(async move {
            crate::tour_viewer::follow_cards(&host, &overlay, on).await;
        });
    });

    let frame_class = if compact {
        "relative flex-1 min-h-0 w-full bg-gray-900 overflow-hidden"
    } else {
        "relative w-full h-[70vh] bg-gray-900 rounded-lg overflow-hidden"
    };
    rsx! {
        div { class: "{frame_class}",
            SplatHost { asset_id: asset_id.clone(), host_id: host_id.clone() }
            if !items.is_empty() {
                AnnotModeToggle {
                    mode: mode_now,
                    has_cards: !cards.is_empty(),
                    on_change: move |m| {
                        mode.set(Some(m));
                        selected.set(None);
                    },
                }
            }
            if show_cards {
                div { key: "{cards_key}",
                    AnnotationCardsOverlay { items: cards.clone(), overlay_id: overlay_id.clone() }
                }
            }
            if let Some(item) = selected.cloned() {
                div { key: "annot-pop-{item.id}",
                    AnnotationPopover { item, on_close: move |_| selected.set(None) }
                }
            }
        }
    }
}
