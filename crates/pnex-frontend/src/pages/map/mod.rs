//! Carte globale POI-first (D35–D39) : vue plein écran de **tous** les POI
//! de l'org, sidebar gauche repliable (recherche, filtres, « tout
//! afficher »), **clustering backend** (D37 — jamais 500 points d'un coup),
//! ajout d'un POI au clic carte (palette emoji), drawer de détail avec
//! section unique **« objets attachés »** (device colonne D36 en 1ʳᵉ ligne,
//! puis arêtes `placed_on` tous kinds confondus — média, tour, dashboard —
//! via le registre D42 et les endpoints `/api/v1/resources/edges`), picker
//! générique ongleté (`components/resource_picker.rs`), et couche GPS live
//! des devices (D38, poll 15 s).
//!
//! Patterns : viewer JS `map_viewer` (badge, jamais panic), boucle de
//! polling `MAP_POLL_ACTIVE` (école Phase A), drawer droite (école
//! media.rs), polling REST 15 s (D31 — pas de WS navigateur), signaux
//! possédés AVANT le rsx et passés **par valeur** aux helpers async.

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use dioxus::prelude::*;
use dioxus_i18n::t;

use crate::api::{self, viz};
use crate::app::Route;
use crate::components::confirm::ConfirmDialog;
use crate::components::icons;
use crate::components::modal::Modal;
use crate::components::poi_preview::{PoiPreviewPanel, PreviewTarget};
use crate::components::poi_tree::PoiTreeSection;
use crate::components::resource_picker::{PickerTab, ResourcePick, ResourcePicker};
use crate::map_viewer::{self, MapItem, MapOpts};
use crate::state::map::OPEN_POI;
use crate::state::toasts;
use crate::state::tours::OPEN_TOUR;
use crate::util::sleep;

const MAP_HOST: &str = "pnex-map-host";
static MAP_POLL_ACTIVE: AtomicBool = AtomicBool::new(false);

/// Couche GPS : rafraîchie tous les ~15 s (école D31 : 50 ticks × 300 ms).
const POSITIONS_POLL_TICKS: u32 = 50;
/// Palette de pictogrammes POI (curatée — capteurs, eau, énergie, accès,
/// risque…) ; sur mobile le champ accepte aussi le clavier emoji natif.
const PALETTE: [&str; 12] = [
    "📍", "🌱", "🌡️", "💧", "⚡", "🚰", "🔥", "🚒", "⚠️", "🚪", "🔧", "📡",
];

/// Centre par défaut (France) tant qu'aucun POI ne guide la vue.
const DEFAULT_CENTER: (f64, f64) = (2.4, 46.6);
const DEFAULT_ZOOM: f64 = 5.0;

#[component]
pub fn Map() -> Element {
    let mut sidebar_open = use_signal(|| true);
    let mut search = use_signal(String::new);
    let mut filter_device = use_signal(|| false);
    let mut filter_position = use_signal(|| false);
    let mut filter_emoji = use_signal(String::new);
    // POI ouvert dans le drawer de détail.
    let mut selected = use_signal(|| None::<String>);

    // Global search deep link (D69): consume the requested POI id once
    // (guard prevents the effect from re-arming — see flows.rs comment).
    let mut deep_link_done = use_signal(|| false);
    use_effect(move || {
        if deep_link_done() {
            return;
        }
        if let Some(id) = OPEN_POI() {
            selected.set(Some(id));
            OPEN_POI.with_mut(|f| *f = None);
            deep_link_done.set(true);
        }
    });
    // Mode « ajouter » : le prochain clic carte ouvre le formulaire.
    let mut add_mode = use_signal(|| false);
    // Coords captées par le clic carte (formulaire de création ouvert).
    let mut pending_add = use_signal(|| None::<(f64, f64)>);
    // Aperçu intégré (read-only) : objet affiché dans le panneau qui couvre
    // la carte, état « afficher la carte » et épingle POI (★).
    let mut preview = use_signal(|| None::<PreviewTarget>);
    let mut preview_hidden = use_signal(|| false);
    let mut pinned = use_signal(|| None::<PreviewTarget>);
    let mut map_failed = use_signal(|| false);
    // Compteur d'événements (création/édition) → refetch liste.
    let mut reload = use_signal(|| 0u32);
    // Compteur de changements de filtres → refetch cluster.
    let mut filters_rev = use_signal(|| 0u32);
    // Dernier zoom connu (clic cluster → zoom +2).
    let mut current_zoom = use_signal(|| DEFAULT_ZOOM);
    // Dernier viewport connu (refetch cluster quand les filtres changent).
    let mut last_view = use_signal(|| None::<(f64, f64, f64, f64, i32)>);
    // Couche GPS live (device_positions, D38).
    let mut positions = use_signal(Vec::<viz::DevicePosition>::new);

    // Liste sidebar : réactive aux filtres + reload (use_resource suit les
    // lectures de signaux de la closure).
    let list_resource = use_resource(move || async move {
        // Création/édition/suppression POI → refetch de la liste.
        reload();
        let filters = current_filters(search, filter_emoji, filter_device, filter_position);
        viz::list_pois(&filters).await
    });

    // Mount + boucle de polling (viewport → cluster ; clics ; GPS 15 s).
    use_effect(move || {
        MAP_POLL_ACTIVE.store(true, Ordering::Relaxed);
        spawn(async move {
            let opts = MapOpts {
                style_url: viz::MAP_STYLE_URL.to_string(),
                center: DEFAULT_CENTER,
                zoom: DEFAULT_ZOOM,
                items: vec![],
            };
            let ok = map_viewer::mount(MAP_HOST, &opts).await;
            map_failed.set(!ok);

            let mut view_seen = 0u64;
            let mut click_seen = 0u64;
            let mut pos_loaded = false;
            let mut filters_seen = 0u64;
            let mut tick = 0u32;
            while MAP_POLL_ACTIVE.load(Ordering::Relaxed) {
                // Viewport (moveend debouncé côté map.js) → refetch cluster.
                if let Some(v) = map_viewer::take_viewport(view_seen).await {
                    view_seen = v.seq;
                    current_zoom.set(v.zoom);
                    last_view.set(Some((v.west, v.south, v.east, v.north, v.zoom as i32)));
                    let filters =
                        current_filters(search, filter_emoji, filter_device, filter_position);
                    redraw_with(
                        &positions,
                        &filters,
                        v.west,
                        v.south,
                        v.east,
                        v.north,
                        v.zoom as i32,
                    )
                    .await;
                }
                // Filtres changés OU données POI modifiées (création/édition/
                // suppression, attach/detach device ou pano) → refetch cluster
                // sur le dernier viewport connu (sinon le pin n'apparaît
                // qu'au rechargement de la page).
                let rev = filters_rev() as u64 + reload() as u64;
                if rev != filters_seen {
                    filters_seen = rev;
                    if let Some((w, s, e, n, z)) = last_view() {
                        let filters =
                            current_filters(search, filter_emoji, filter_device, filter_position);
                        redraw_with(&positions, &filters, w, s, e, n, z).await;
                    }
                }
                // Clics (items et carte nue).
                if let Some(click) = map_viewer::take_click(click_seen).await {
                    click_seen = click.seq;
                    handle_click(selected, pending_add, current_zoom, add_mode, click).await;
                }
                // Couche GPS live (D38) — poll ~15 s.
                tick += 1;
                if tick.is_multiple_of(POSITIONS_POLL_TICKS) {
                    if let Ok(rows) = viz::device_positions().await {
                        positions.set(rows.clone());
                        if !pos_loaded {
                            // Première couche GPS : arrivée après le premier
                            // viewport → un redessin pour les afficher.
                            pos_loaded = true;
                            if let Some((w, s, e, n, z)) = last_view() {
                                let filters = current_filters(
                                    search,
                                    filter_emoji,
                                    filter_device,
                                    filter_position,
                                );
                                redraw_with(&positions, &filters, w, s, e, n, z).await;
                            }
                        }
                    }
                }
                sleep(Duration::from_millis(300)).await;
            }
        });
    });

    use_drop(move || {
        MAP_POLL_ACTIVE.store(false, Ordering::Relaxed);
        map_viewer::unmount(MAP_HOST);
    });

    // Valeurs possédées pour le rsx (pas de borrow traversant les closures).
    let list_page = list_resource.read().clone().and_then(Result::ok);
    let pois = list_page
        .as_ref()
        .map(|p| p.results.clone())
        .unwrap_or_default();
    let total = list_page.as_ref().map(|p| p.count).unwrap_or(0);
    let positions_list = positions.read().clone();
    let add_active = add_mode();

    rsx! {
        div {
            class: "flex h-[calc(100vh-4rem)] lg:h-screen overflow-hidden bg-gray-100",
            // ── Sidebar gauche repliable ──────────────────────────
            if sidebar_open() {
                aside { class: "w-80 shrink-0 bg-white border-r border-gray-200 flex flex-col",
                    // En-tête + repli.
                    div { class: "flex items-center justify-between px-4 py-3 border-b border-gray-200",
                        h1 { class: "text-base font-semibold text-gray-900", {t!("viz-map-title")} }
                        button {
                            class: "p-1.5 text-gray-500 hover:text-gray-900 rounded-lg hover:bg-gray-100 transition-colors",
                            title: t!("poi-collapse"),
                            onclick: move |_| sidebar_open.set(false),
                            icons::X { class: "h-5 w-5" }
                        }
                    }
                    // Recherche.
                    div { class: "px-4 py-3 border-b border-gray-100",
                        input {
                            class: "w-full px-3 py-2 border border-gray-300 rounded-lg text-sm",
                            r#type: "search",
                            placeholder: t!("poi-search-placeholder"),
                            value: "{search}",
                            oninput: move |e| {
                                search.set(e.value());
                                filters_rev += 1;
                            },
                        }
                    }
                    // Filtres.
                    div { class: "px-4 py-3 border-b border-gray-100 space-y-2",
                        label { class: "flex items-center gap-2 text-sm text-gray-700",
                            input {
                                r#type: "checkbox",
                                checked: filter_device(),
                                onchange: move |_| {
                                    filter_device.toggle();
                                    filters_rev += 1;
                                },
                            }
                            {t!("poi-filter-device")}
                        }
                        label { class: "flex items-center gap-2 text-sm text-gray-700",
                            input {
                                r#type: "checkbox",
                                checked: filter_position(),
                                onchange: move |_| {
                                    filter_position.toggle();
                                    filters_rev += 1;
                                },
                            }
                            {t!("poi-filter-gps")}
                        }
                        div { class: "flex items-center gap-2",
                            span { class: "text-sm text-gray-700 shrink-0", {t!("poi-filter-emoji")} }
                            select {
                                aria_label: t!("poi-filter-emoji"),
                                class: "flex-1 px-2 py-1.5 border border-gray-300 rounded-lg text-sm bg-white",
                                value: "{filter_emoji}",
                                onchange: move |e| {
                                    filter_emoji.set(e.value());
                                    filters_rev += 1;
                                },
                                option { value: "", {t!("poi-filter-emoji-all")} }
                                for emoji in PALETTE {
                                    option { value: "{emoji}", "{emoji}" }
                                }
                            }
                        }
                        button {
                            class: "w-full px-3 py-1.5 text-sm text-blue-700 border border-blue-200 rounded-lg hover:bg-blue-50 transition-colors",
                            onclick: move |_| {
                                search.set(String::new());
                                filter_device.set(false);
                                filter_position.set(false);
                                filter_emoji.set(String::new());
                                filters_rev += 1;
                            },
                            {t!("poi-show-all")}
                        }
                    }
                    // Liste des POI.
                    div { class: "flex-1 overflow-y-auto",
                        if pois.is_empty() {
                            div { class: "p-6 text-center text-sm text-gray-500", {t!("poi-empty")} }
                        } else {
                            for poi in pois.iter() {
                                PoiRow {
                                    key: "{poi.id}",
                                    poi: poi.clone(),
                                    positions: positions_list.clone(),
                                    on_select: move |id: String| selected.set(Some(id)),
                                }
                            }
                        }
                    }
                    div { class: "px-4 py-2 border-t border-gray-100 text-xs text-gray-500",
                        {t!("poi-count", count : total)}
                    }
                }
            }
            // ── Carte plein écran ─────────────────────────────────
            div { class: "relative flex-1",
                div {
                    id: MAP_HOST,
                    class: "absolute inset-0",
                    style: "width: 100%; height: 100%;",
                }
                if map_failed() {
                    MapErrorBadge {}
                }
                // Boutons flottants (repli sidebar + mode ajout).
                div { class: "absolute top-3 left-3 z-10 flex gap-2",
                    if !sidebar_open() {
                        button {
                            class: "p-2.5 bg-white rounded-lg shadow border border-gray-200 text-gray-600 hover:text-gray-900 transition-colors",
                            title: t!("poi-expand"),
                            onclick: move |_| sidebar_open.set(true),
                            icons::Menu { class: "h-5 w-5" }
                        }
                    }
                    button {
                        class: if add_active { "px-3 py-2.5 text-sm font-semibold text-white bg-blue-600 rounded-lg shadow border border-blue-700 transition-colors" } else { "px-3 py-2.5 text-sm font-semibold text-blue-700 bg-white rounded-lg shadow border border-blue-200 hover:bg-blue-50 transition-colors" },
                        onclick: move |_| add_mode.toggle(),
                        if add_active {
                            {t!("poi-add-cancel")}
                        } else {
                            {t!("poi-add")}
                        }
                    }
                }
                if add_active {
                    div { class: "absolute top-16 left-3 z-10 px-3 py-1.5 text-xs font-medium text-white bg-blue-600/90 rounded-lg shadow",
                        {t!("poi-add-hint")}
                    }
                }
            }
            // ── Drawer détail POI ─────────────────────────────────
            if let Some(id) = selected() {
                PoiDetail {
                    key: "{id}",
                    poi_id: id,
                    preview,
                    preview_hidden,
                    pinned,
                    on_close: move |_| {
                        selected.set(None);
                        // Aperçu + épingle appartiennent au POI ouvert.
                        preview.set(None);
                        preview_hidden.set(false);
                        pinned.set(None);
                    },
                    on_changed: move |_| reload += 1,
                }
            }
            // ── Modale de création (coords du clic carte) ────────
            if let Some((lat, lon)) = pending_add() {
                PoiFormModal {
                    key: "create-{lat}-{lon}",
                    initial: None,
                    coords: (lat, lon),
                    on_saved: move |_| {
                        pending_add.set(None);
                        add_mode.set(false);
                        reload += 1;
                    },
                    on_close: move |_| pending_add.set(None),
                }
            }
            // ── Modales viewers (média + tour) ────────────────────
            // (supprimées — l'aperçu intégré remplace modals et navigation)
        }
    }
}

mod detail;
mod filters;
mod form;

use detail::PoiDetail;
use filters::{current_filters, handle_click, redraw_with, PoiRow};
use form::PoiFormModal;
// ───────────────────── viewer média attaché (modal) ─────────────────────

// ─────────────────────────── badge d'erreur carte ─────────────────────────

#[component]
pub(super) fn MapErrorBadge() -> Element {
    let mut message = use_signal(|| None::<String>);
    use_effect(move || {
        spawn(async move {
            sleep(Duration::from_millis(5200)).await;
            let err = map_viewer::error(MAP_HOST).await;
            message.set(err);
        });
    });
    rsx! {
        div { class: "absolute top-3 right-3 z-10 max-w-sm px-3 py-2 bg-amber-50 border border-amber-200 rounded-lg shadow",
            div { class: "text-sm font-medium text-amber-800", {t!("viz-map-unavailable")} }
            if let Some(err) = message() {
                div { class: "text-xs text-amber-700 mt-0.5 break-all", "{err}" }
            }
        }
    }
}
