//! POI-first global map (D35–D39): full-screen view of **all** the org's
//! POIs, collapsible left sidebar (search, filters, "show
//! all"), **backend clustering** (D37 — never 500 points at once),
//! adding a POI on map click (emoji palette), detail drawer with a
//! single **"attached objects"** section (D36 column device on the 1st row,
//! then `placed_on` edges of all kinds — media, tour, dashboard —
//! via the D42 registry and the `/api/v1/resources/edges` endpoints), generic
//! tabbed picker (`components/resource_picker.rs`), and live GPS layer
//! of the devices (D38, poll 15 s).
//!
//! Patterns: JS viewer `map_viewer` (badge, never panic), polling loop
//! `MAP_POLL_ACTIVE` (Phase A pattern), right drawer (media.rs
//! pattern), REST polling 15 s (D31 — no browser WS), signals
//! owned BEFORE the rsx and passed **by value** to the async helpers.

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use dioxus::prelude::*;
use dioxus_i18n::t;

use crate::api::{self, viz};
use crate::app::Route;
use crate::components::confirm::ConfirmDialog;
use crate::components::crud::pager::{ListPager, PAGE_SIZE};
use crate::components::icons;
use crate::components::map_panel::{MapPanel, MapPanelToggle};
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

/// GPS layer: refreshed every ~15 s (D31 pattern: 50 ticks × 300 ms).
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

/// POI pictogram palette (curated — sensors, water, energy, access,
/// risk…); on mobile the field also accepts the native emoji keyboard.
const PALETTE: [&str; 12] = [
    "📍", "🌱", "🌡️", "💧", "⚡", "🚰", "🔥", "🚒", "⚠️", "🚪", "🔧", "📡",
];

/// Default center (France) as long as no POI guides the view.
pub(crate) const DEFAULT_CENTER: (f64, f64) = (2.4, 46.6);
pub(crate) const DEFAULT_ZOOM: f64 = 5.0;

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
    // Sidebar list page (server-side, D14); back to the first page whenever
    // a filter changes.
    let mut poi_page = use_signal(|| 0i64);
    use_effect(move || {
        let _ = (search(), filter_emoji(), filter_device(), filter_position());
        if *poi_page.peek() != 0 {
            poi_page.set(0);
        }
    });
    // POI open in the detail drawer.
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
    // "Add" mode: the next map click opens the form.
    let mut add_mode = use_signal(|| false);
    // Coords captured by the map click (creation form open).
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
    // Event counter (create/edit) → list refetch.
    let mut reload = use_signal(|| 0u32);
    // Filter change counter → cluster refetch.
    let mut filters_rev = use_signal(|| 0u32);
    // Dernier zoom connu (clic cluster → zoom +2).
    let mut current_zoom = use_signal(|| DEFAULT_ZOOM);
    // Last known viewport (cluster refetch when the filters change).
    let mut last_view = use_signal(|| None::<(f64, f64, f64, f64, i32)>);
    // Couche GPS live (device_positions, D38).
    let mut positions = use_signal(Vec::<viz::DevicePosition>::new);
    // Map points status (slow networks: the user sees that points are on
    // their way, or that they failed and are retried).
    let mut points_status = use_signal(|| PointsStatus::Loading);

    // Sidebar list: reactive to filters + reload (use_resource tracks the
    // signal reads of the closure).
    let list_resource = use_resource(move || async move {
        // POI create/edit/delete → list refetch.
        reload();
        let filters = viz::PoiFilters {
            limit: Some(PAGE_SIZE),
            offset: Some(poi_page() * PAGE_SIZE),
            ..current_filters(search, filter_emoji, filter_device, filter_position)
        };
        viz::list_pois(&filters).await
    });

    // Mount + polling loop (viewport → cluster; clicks; GPS 15 s).
    use_effect(move || {
        MAP_POLL_ACTIVE.store(true, Ordering::Relaxed);
        spawn(async move {
            let maps = crate::api::geo::basemaps().await.unwrap_or_default();
            let chosen = pick_basemap(&maps);
            basemap.set(chosen.map(|b| b.id));
            let style_url = chosen.map(|b| b.style_url.clone()).unwrap_or_default();
            let style_url_dark = chosen.and_then(|b| b.style_url_dark.clone());
            basemaps.set(maps);
            let opts = MapOpts {
                style_url,
                style_url_dark,
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
                // Viewport (moveend debounced in map.js) → cluster refetch.
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
                // Filters changed OR POI data modified (create/edit/
                // delete, attach/detach device or pano) → cluster refetch
                // on the last known viewport (otherwise the pin only shows
                // up on page reload).
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
                // Clicks (items and bare map).
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
                            // First GPS layer: arrived after the first
                            // viewport → one redraw to show them.
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

    // Owned values for the rsx (no borrow crossing the closures).
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
                    class: "pnex-backdrop-in absolute inset-0 z-20 bg-black/20 lg:hidden",
                    onclick: move |_| sidebar_open.set(false),
                }
                MapPanel {
                    title: t!("viz-map-title").to_string(),
                    page_heading: true,
                    header_extra: rsx! {
                        Link {
                            to: Route::Sites {},
                            class: "mr-1 text-xs font-medium text-signal-700 hover:underline",
                            {t!("map-open-sites")}
                        }
                    },
                    on_close: move |_| sidebar_open.set(false),
                    search,
                    search_placeholder: t!("poi-search-placeholder").to_string(),
                    on_search: move |_| filters_rev += 1,
                    filters: rsx! {
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
                            class: "w-full px-3 py-1.5 text-sm text-signal-700 border border-signal-200 rounded-lg hover:bg-signal-50 transition-colors",
                            onclick: move |_| {
                                search.set(String::new());
                                filter_device.set(false);
                                filter_position.set(false);
                                filter_emoji.set(String::new());
                                filters_rev += 1;
                            },
                            {t!("poi-show-all")}
                        }
                    },
                    footer: rsx! {
                        {t!("poi-count", count : total)}
                        ListPager { count: total, page: poi_page }
                    },
                    if list_loading {
                        div { class: "p-6 flex justify-center",
                            Gerbe { size: 22 }
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
            }
            // ── Full-screen map ─────────────────────────────────
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
                // Floating tools, as on a case map: the round `+` (add
                // mode), the list toggle, then the address search.
                div { class: "absolute top-4 left-4 z-10 flex items-center gap-2",
                    button {
                        r#type: "button",
                        class: crate::components::ui::BTN_FAB,
                        title: if add_active { t!("poi-add-cancel") } else { t!("poi-add") },
                        aria_label: if add_active { t!("poi-add-cancel") } else { t!("poi-add") },
                        "aria-pressed": add_active,
                        onclick: move |_| add_mode.toggle(),
                        if add_active {
                            icons::X { class: "h-5 w-5" }
                        } else {
                            icons::Plus { class: "h-5 w-5" }
                        }
                    }
                    if !sidebar_open() {
                        MapPanelToggle { onclick: move |_| sidebar_open.set(true) }
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
                            Gerbe { size: 16 }
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
                    div { class: "absolute top-16 left-3 z-10 px-3 py-1.5 text-xs font-medium text-white bg-signal-600/90 rounded-lg shadow",
                        {t!("poi-add-hint")}
                    }
                }
            }
            // ── POI detail drawer ─────────────────────────────────
            if let Some(id) = selected() {
                PoiDetail {
                    key: "{id}",
                    poi_id: id,
                    preview,
                    pinned,
                    on_close: move |_| {
                        selected.set(None);
                        // Preview + pin belong to the open POI.
                        preview.set(None);
                        pinned.set(None);
                    },
                    on_changed: move |_| reload += 1,
                }
            }
            // ── Creation modal (coords of the map click) ────────
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
            // ── Viewer modals (media + tour) ────────────────────
            // (removed — the built-in preview replaces modals and navigation)
        }
    }
}

mod detail;
mod filters;
mod form;
mod search;
mod sites;

pub use sites::Sites;

use crate::components::gerbe::Gerbe;
use detail::PoiDetail;
use filters::{current_filters, handle_click, redraw_with, PoiRow};
use form::PoiFormModal;
use search::AddressSearch;
// ───────────────────── attached media viewer (modal) ─────────────────────

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
pub(crate) fn pick_basemap(maps: &[pnex_core::geo::Basemap]) -> Option<&pnex_core::geo::Basemap> {
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
                    class: "text-signal-600 hover:underline",
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
    let Some((style, style_dark)) = basemaps
        .read()
        .iter()
        .find(|b| b.id == id)
        .map(|b| (b.style_url.clone(), b.style_url_dark.clone()))
    else {
        return;
    };
    crate::storage::local().set(&basemap_key(), &id.to_string());
    basemap.set(Some(id));
    spawn(async move {
        let opts = MapOpts {
            style_url: style,
            style_url_dark: style_dark,
            center: DEFAULT_CENTER,
            zoom: DEFAULT_ZOOM,
            items: vec![],
        };
        map_viewer::mount(MAP_HOST, &opts).await;
    });
}
