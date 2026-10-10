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
/// Retry delay of a failed points fetch (10 ticks × 300 ms = 3 s).
const POINTS_RETRY_TICKS: u32 = 10;

/// Loading state of the map points (cluster fetch).
#[derive(Clone, Copy, PartialEq)]
enum PointsStatus {
    Loading,
    Ready,
    Failed,
}

impl PointsStatus {
    fn from_fetch(ok: bool) -> Self {
        if ok {
            Self::Ready
        } else {
            Self::Failed
        }
    }
}

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
    // Phone: the POI panel starts closed (it would cover the map) and
    // opens as an overlay; from lg up it sits open beside the map.
    let mut sidebar_open = use_signal(|| false);
    use_effect(move || {
        spawn(async move {
            let wide = dioxus::document::eval("return window.innerWidth >= 1024")
                .await
                .ok()
                .and_then(|v| v.as_bool())
                .unwrap_or(true);
            if wide {
                sidebar_open.set(true);
            }
        });
    });
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
    // Address picked in the address search, prefilling the creation form.
    let mut suggested = use_signal(|| None::<pnex_core::geo::GeocodeResult>);
    // Embedded read-only preview: object shown in the panel covering the
    // map, and the POI pin (★).
    let mut preview = use_signal(|| None::<PreviewTarget>);
    let mut pinned = use_signal(|| None::<PreviewTarget>);
    let mut map_failed = use_signal(|| false);
    // Basemaps of the org (geo-layers.md L24–L26) and the one shown.
    let mut basemaps = use_signal(Vec::<pnex_core::geo::Basemap>::new);
    let mut basemap = use_signal(|| None::<uuid::Uuid>);
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
    // Map points status (slow networks: the user sees that points are on
    // their way, or that they failed and are retried).
    let mut points_status = use_signal(|| PointsStatus::Loading);

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
            let maps = crate::api::geo::basemaps().await.unwrap_or_default();
            let chosen = pick_basemap(&maps);
            basemap.set(chosen.map(|b| b.id));
            let style_url = chosen.map(|b| b.style_url.clone()).unwrap_or_default();
            basemaps.set(maps);
            let opts = MapOpts {
                style_url,
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
            let mut failed_at = 0u32;
            while MAP_POLL_ACTIVE.load(Ordering::Relaxed) {
                // Viewport (moveend debouncé côté map.js) → refetch cluster.
                if let Some(v) = map_viewer::take_viewport(view_seen).await {
                    view_seen = v.seq;
                    current_zoom.set(v.zoom);
                    last_view.set(Some((v.west, v.south, v.east, v.north, v.zoom as i32)));
                    let filters =
                        current_filters(search, filter_emoji, filter_device, filter_position);
                    points_status.set(PointsStatus::Loading);
                    let ok = redraw_with(
                        &positions,
                        &filters,
                        v.west,
                        v.south,
                        v.east,
                        v.north,
                        v.zoom as i32,
                    )
                    .await;
                    points_status.set(PointsStatus::from_fetch(ok));
                    failed_at = tick;
                }
                // Filtres changés OU données POI modifiées (création/édition/
                // suppression, attach/detach device ou pano) → refetch cluster
                // sur le dernier viewport connu (sinon le pin n'apparaît
                // qu'au rechargement de la page).
                let rev = filters_rev() as u64 + reload() as u64;
                // A failed fetch is retried every few seconds on the same view.
                let retry = points_status() == PointsStatus::Failed
                    && tick.wrapping_sub(failed_at) >= POINTS_RETRY_TICKS;
                if rev != filters_seen || retry {
                    filters_seen = rev;
                    if let Some((w, s, e, n, z)) = last_view() {
                        let filters =
                            current_filters(search, filter_emoji, filter_device, filter_position);
                        points_status.set(PointsStatus::Loading);
                        let ok = redraw_with(&positions, &filters, w, s, e, n, z).await;
                        points_status.set(PointsStatus::from_fetch(ok));
                        failed_at = tick;
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
    // `None` while the first list fetch is in flight: "loading", not "empty".
    let list_loading = list_resource.read().is_none();
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
            class: "relative flex h-[calc(100vh-4rem)] lg:h-screen overflow-hidden bg-gray-100",
            // ── Sidebar gauche repliable ──────────────────────────
            if sidebar_open() {
                // Phone backdrop: tap outside the overlay panel to close it.
                div {
                    class: "absolute inset-0 z-20 bg-black/20 lg:hidden",
                    onclick: move |_| sidebar_open.set(false),
                }
                aside { class: "absolute inset-y-0 left-0 z-30 w-80 max-w-[85vw] shrink-0 bg-white border-r border-gray-200 flex flex-col shadow-xl lg:static lg:z-auto lg:max-w-none lg:shadow-none",
                    // En-tête + repli.
                    div { class: "flex items-center justify-between px-4 py-3 border-b border-gray-200",
                        h1 { class: "text-base font-semibold text-gray-900", {t!("viz-map-title")} }
                        Link {
                            to: Route::Sites {},
                            class: "ml-auto mr-1 text-xs font-medium text-blue-700 hover:underline",
                            {t!("map-open-sites")}
                        }
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
                        if list_loading {
                            div { class: "p-6 flex justify-center",
                                span { class: "animate-spin rounded-full h-5 w-5 border-b-2 border-blue-600" }
                            }
                        } else if pois.is_empty() {
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
                if !map_failed() {
                    BasemapControl { basemaps, basemap }
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
                    AddressSearch { pending_add, suggested }
                }
                // Points status pill (top centre): spinner while loading,
                // amber while failing (retried automatically).
                match (map_failed(), points_status()) {
                    (false, PointsStatus::Loading) => rsx! {
                        div {
                            role: "status",
                            class: "absolute top-3 left-1/2 -translate-x-1/2 z-10 inline-flex items-center gap-2 px-3 py-1.5 text-xs font-medium text-gray-700 bg-white/95 rounded-full shadow border border-gray-200",
                            span { class: "animate-spin rounded-full h-3.5 w-3.5 border-b-2 border-blue-600" }
                            {t!("poi-points-loading")}
                        }
                    },
                    (false, PointsStatus::Failed) => rsx! {
                        div {
                            role: "status",
                            class: "absolute top-3 left-1/2 -translate-x-1/2 z-10 px-3 py-1.5 text-xs font-medium text-amber-800 bg-amber-50 rounded-full shadow border border-amber-200",
                            {t!("poi-points-failed")}
                        }
                    },
                    _ => rsx! {},
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
                    pinned,
                    on_close: move |_| {
                        selected.set(None);
                        // Aperçu + épingle appartiennent au POI ouvert.
                        preview.set(None);
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
                    suggested: suggested(),
                    on_saved: move |_| {
                        pending_add.set(None);
                        suggested.set(None);
                        add_mode.set(false);
                        reload += 1;
                    },
                    on_close: move |_| {
                        pending_add.set(None);
                        suggested.set(None);
                    },
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
mod search;
mod sites;

pub use sites::Sites;

use detail::PoiDetail;
use filters::{current_filters, handle_click, redraw_with, PoiRow};
use form::PoiFormModal;
use search::AddressSearch;
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

/// Storage key of the basemap chosen in the current org.
fn basemap_key() -> String {
    let org = crate::state::org::current().unwrap_or_default();
    format!("{}{org}", crate::storage::KEY_BASEMAP_PREFIX)
}

/// The user's stored choice if it still exists, else the org default,
/// else the first basemap (L25).
fn pick_basemap(maps: &[pnex_core::geo::Basemap]) -> Option<&pnex_core::geo::Basemap> {
    use crate::storage::KeyValueStorage;
    let stored = crate::storage::local()
        .get(&basemap_key())
        .and_then(|s| s.parse::<uuid::Uuid>().ok());
    stored
        .and_then(|id| maps.iter().find(|b| b.id == id))
        .or_else(|| maps.iter().find(|b| b.is_default))
        .or_else(|| maps.first())
}

/// Basemap switcher (more than one basemap) or "no basemap configured"
/// notice linking to the org page (none).
#[component]
fn BasemapControl(
    basemaps: Signal<Vec<pnex_core::geo::Basemap>>,
    basemap: Signal<Option<uuid::Uuid>>,
) -> Element {
    let maps = basemaps();
    if maps.is_empty() {
        return rsx! {
            div { class: "absolute bottom-8 left-1/2 -translate-x-1/2 z-10 px-3 py-1.5 text-xs font-medium text-gray-700 bg-white/95 rounded-full shadow border border-gray-200",
                {t!("geo-basemap-none")}
                " "
                Link {
                    to: Route::OrgsCurrent {},
                    class: "text-blue-600 hover:underline",
                    {t!("geo-basemap-configure")}
                }
            }
        };
    }
    if maps.len() < 2 {
        return rsx! {};
    }
    // `selected` per option: a `value` on the select alone shows the first
    // option (dioxus).
    let current = basemap();
    rsx! {
        select {
            class: "absolute top-3 right-3 z-10 px-2 py-1.5 text-sm bg-white rounded-lg shadow border border-gray-200",
            aria_label: t!("geo-basemap-label"),
            onchange: move |e| switch_basemap(basemaps, basemap, &e.value()),
            for b in maps {
                option { value: "{b.id}", selected: current == Some(b.id), "{b.name}" }
            }
        }
    }
}

/// Stores the chosen basemap and remounts the map on its style (markers
/// come back with the first viewport event of the new map).
fn switch_basemap(
    basemaps: Signal<Vec<pnex_core::geo::Basemap>>,
    mut basemap: Signal<Option<uuid::Uuid>>,
    value: &str,
) {
    use crate::storage::KeyValueStorage;
    let Ok(id) = value.parse::<uuid::Uuid>() else {
        return;
    };
    let Some(style) = basemaps
        .read()
        .iter()
        .find(|b| b.id == id)
        .map(|b| b.style_url.clone())
    else {
        return;
    };
    crate::storage::local().set(&basemap_key(), &id.to_string());
    basemap.set(Some(id));
    spawn(async move {
        let opts = MapOpts {
            style_url: style,
            center: DEFAULT_CENTER,
            zoom: DEFAULT_ZOOM,
            items: vec![],
        };
        map_viewer::mount(MAP_HOST, &opts).await;
    });
}
