//! Media library (D21) — filterable list (kind/search, D14), octet-stream
//! upload (local folder or Android camera capture), signal-based detail
//! (never a dynamic route — dioxus #2784 pitfall), previews: photo `<img>`
//! via blob URL, pannellum panorama + gsplat.js splat via `media_viewer`
//! (tron.rs pattern). Writes are owner/admin only (server enforces, UI
//! hides — devices.rs school).
//!
//! V1: no list thumbnails (icon per kind); `.ksplat`/`.spz` storable but
//! without preview (badge); native blob support (webview, CORS) → previews
//! web-only in V1 (cf. docs/architecture/media.md).
//!
//! Detail view (redesign 2026-09-19): full-page two-column layout —
//! preview + metadata strip on the left, info/labels/versions cards on
//! the right (the versions drawer is gone). "View" previews a non-current
//! version via `/versions/{n}/content` (read-only blob switch).

use dioxus::prelude::*;
use dioxus_i18n::t;

use crate::api;
use crate::api::media::{MediaAsset, MediaFilters, MediaKind, UploadParams};
use crate::components::badges::date_label;
use crate::components::confirm::ConfirmDialog;
use crate::components::crud::filters::{FilterBar, RefreshButton, SearchInput};
use crate::components::crud::layout::{ListLayout, PRIMARY_BTN};
use crate::components::crud::pager::{ListPager, PAGE_SIZE};
use crate::components::crud::states::ListStates;
use crate::components::crud::table::{Column, DataTable, RowKey};
use crate::components::icons;
use crate::components::labels_editor::LabelsEditor;
use crate::components::modal::Modal;
use crate::state::media::OPEN_MEDIA;
use crate::state::{org, session, toasts};
use crate::util::{media_blob_url, sleep, trigger_download};

/// User role in the current org (devices.rs school).
fn current_role() -> Option<String> {
    let user = session::user()?;
    let org_id = org::current()?;
    user.orgs
        .iter()
        .find(|m| m.id == org_id)
        .map(|m| m.role.clone())
}

/// Human-readable size (B/KB/MB/GB) for the list Size column.
fn format_size(bytes: i64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} B")
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

#[component]
pub fn Media() -> Element {
    let mut reload = use_signal(|| 0u32);
    let mut selected = use_signal(|| None::<String>);

    // Global search deep link (D69): consume the requested asset id once
    // (guard prevents the effect from re-arming — see flows.rs comment).
    let mut deep_link_done = use_signal(|| false);
    use_effect(move || {
        if deep_link_done() {
            return;
        }
        if let Some(id) = OPEN_MEDIA() {
            selected.set(Some(id));
            OPEN_MEDIA.with_mut(|f| *f = None);
            deep_link_done.set(true);
        }
    });
    let mut filter_kind = use_signal(|| "all".to_string());
    let search = use_signal(String::new);
    // D42: effective label filter ("name" or "name:value").
    let filter_label = use_signal(String::new);
    let mut page = use_signal(|| 0i64);
    let mut upload_open = use_signal(|| false);
    // Camera capture in flight (detached upload) — waiting indicator.
    let mut capture_running = use_signal(crate::capture::capture_in_progress);
    // Take 360 overlay (Android) open.
    let mut take360_open = use_signal(|| false);

    let can_write = current_role().is_some_and(|role| crate::state::org::role_can_write(&role));

    let list = use_resource(move || {
        let filters = MediaFilters {
            label: {
                let value = filter_label().trim().to_string();
                if value.is_empty() {
                    None
                } else {
                    Some(value)
                }
            },
            kinds: match filter_kind().as_str() {
                "all" => Vec::new(),
                "photo" => vec![MediaKind::Photo],
                "panorama" => vec![MediaKind::Panorama],
                "floorplan" => vec![MediaKind::Floorplan],
                "model" => vec![MediaKind::Model],
                _ => vec![MediaKind::Splat],
            },
            search: {
                let value = search().trim().to_string();
                if value.is_empty() {
                    None
                } else {
                    Some(value)
                }
            },
            limit: Some(PAGE_SIZE),
            offset: Some(page() * PAGE_SIZE),
        };
        async move {
            let _ = reload();
            api::media::list(&filters).await
        }
    });

    // Capture result polling (500 ms — indicator reactivity): the detached
    // watcher has no valid dioxus scope (pure atomics on the capture side)
    // — the toast and refresh are emitted HERE, in a scope with context.
    use_effect(move || {
        spawn(async move {
            let mut last_seq = crate::capture::capture_state().0;
            let mut last_seq360 =
                crate::capture360::state::RESULT_SEQ.load(std::sync::atomic::Ordering::Relaxed);
            loop {
                sleep(std::time::Duration::from_millis(500)).await;
                let (seq, result) = crate::capture::capture_state();
                if seq != last_seq {
                    last_seq = seq;
                    capture_running.set(false);
                    if result == 1 {
                        toasts::success(t!("media-camera-captured"));
                    } else {
                        toasts::error(t!("media-camera-failed"));
                    }
                    reload.with_mut(|r| *r += 1);
                }

                // Take 360: pipeline result (detached stitch + upload) —
                // same polling school, 3 codes (1 OK, 2 stitch KO, 3 upload
                // KO). THIS component closes the overlay: its internal loop
                // may be dead by then (15% frozen screen seen on device),
                // the page polling stays alive.
                let seq360 =
                    crate::capture360::state::RESULT_SEQ.load(std::sync::atomic::Ordering::Relaxed);
                if seq360 != last_seq360 {
                    last_seq360 = seq360;
                    let code = crate::capture360::state::RESULT_CODE
                        .load(std::sync::atomic::Ordering::Relaxed);
                    if code == 1 {
                        toasts::success(t!("media-take360-done"));
                    } else {
                        toasts::error(t!("media-take360-failed"));
                    }
                    take360_open.set(false);
                    reload.with_mut(|r| *r += 1);
                }
            }
        });
    });

    // Synchronous resource read (CRUD socle doctrine).
    let (list_state, is_empty, count, rows) = match &*list.value().read() {
        None => (None, false, 0, Vec::new()),
        Some(Ok(paged)) => (
            Some(Ok(())),
            paged.results.is_empty(),
            paged.count,
            paged.results.clone(),
        ),
        Some(Err(err)) => (Some(Err(err.clone())), false, 0, Vec::new()),
    };

    // Table columns — the whole row is clickable (detail). No thumbnails:
    // a full blob per row would cost one fetch per line (V1, icons only —
    // a thumbnail endpoint is future work).
    let columns = vec![
        Column::new(t!("media-col-name").to_string(), |asset: &MediaAsset| {
            rsx! { {asset.name.clone()} }
        })
        .with_td_class("font-medium text-gray-900"),
        Column::new(t!("media-col-type").to_string(), |asset: &MediaAsset| {
            rsx! { {kind_label(asset)} }
        })
        .with_td_class("text-gray-600"),
        Column::new(t!("media-col-size").to_string(), |asset: &MediaAsset| {
            rsx! { {format_size(asset.size_bytes)} }
        })
        .with_td_class("text-gray-600"),
        Column::new(
            t!("media-col-versions").to_string(),
            |asset: &MediaAsset| {
                rsx! { "{asset.versions_count}" }
            },
        ),
        Column::new(t!("media-col-updated").to_string(), |asset: &MediaAsset| {
            rsx! { {date_label(&asset.updated_at)} }
        })
        .with_td_class("text-gray-500 text-sm"),
    ];

    rsx! {
        // Detail-first: an open asset renders as a full page (breadcrumb
        // header + two-column layout) — the list shell only renders in
        // list mode.
        if let Some(id) = selected() {
            MediaDetail {
                key: "detail-{id}",
                asset_id: id,
                can_write,
                on_close: move |_| selected.set(None),
                on_upload: move |_| upload_open.set(true),
                on_changed: move |_| reload.with_mut(|r| *r += 1),
            }
        } else {
            ListLayout {
            title: t!("media-title").to_string(),
            subtitle: Some(t!("media-subtitle").to_string()),
            can_write: can_write,
            add_label: Some(t!("media-upload").to_string()),
            on_add: move |_| upload_open.set(true),
            // Multi-button header (camera capture, take360) — socle actions
            // slot; upload stays the primary "add" button.
            actions: rsx! {
                // Camera upload in flight: waiting indicator (the detached
                // watcher cannot touch the UI itself).
                if capture_running() {
                    div { class: "flex items-center gap-2 text-sm text-gray-600",
                        span { class: "animate-spin inline-block rounded-full h-4 w-4 border-b-2 border-blue-600" }
                        {t!("media-upload-progress")}
                    }
                }
                // Native Android capture (system camera intent) — button
                // rendered outside wasm only.
                if !cfg!(target_arch = "wasm32") {
                    button {
                        class: "px-4 py-2 bg-gray-700 text-white rounded-lg hover:bg-gray-800 transition-colors text-sm font-medium",
                        onclick: move |_| {
                            let msgs = crate::capture::CaptureMessages {
                                launched: t!("media-camera-launched"),
                                captured: t!("media-camera-captured"),
                                failed: t!("media-camera-failed"),
                                permission_denied: t!("media-camera-permission-denied"),
                            };
                            if crate::capture::capture_and_upload(msgs) {
                                capture_running.set(true);
                            } else {
                                toasts::error(t!("media-camera-failed"));
                            }
                        },
                        icons::Camera { class: "h-4 w-4 inline mr-1" }
                        {t!("media-camera")}
                    }
                }
                // Take 360: guided panorama capture, Android + owner/admin
                // (getUserMedia preview + on-device stitching — cf.
                // capture360/).
                if cfg!(target_os = "android") && can_write {
                    button {
                        class: "px-4 py-2 bg-gray-700 text-white rounded-lg hover:bg-gray-800 transition-colors text-sm font-medium",
                        disabled: capture_running(),
                        onclick: move |_| take360_open.set(true),
                        icons::RefreshCw { class: "h-4 w-4 inline mr-1" }
                        {t!("media-take360")}
                    }
                }
            },
            // Filters: kind + search + label (D42). The SearchInput switch
            // makes filtering live as you type (canonical socle behavior —
            // the old onchange only filtered on blur/Enter).
            FilterBar {
                select {
                    class: "px-3 py-2 border border-gray-300 rounded-lg text-sm bg-white",
                    value: "{filter_kind}",
                    onchange: move |evt| {
                        filter_kind.set(evt.value());
                        page.set(0);
                    },
                    option { value: "all", {t!("media-filter-all")} }
                    option { value: "photo", {t!("media-kind-photo")} }
                    option { value: "panorama", {t!("media-kind-panorama")} }
                    option { value: "splat", {t!("media-kind-splat")} }
                    option { value: "floorplan", {t!("media-kind-floorplan")} }
                    option { value: "model", {t!("media-kind-model")} }
                }
                SearchInput {
                    placeholder: t!("media-search-placeholder").to_string(),
                    value: search,
                    on_submit: move |_| {
                        page.set(0);
                    },
                }
                // D42: effective label filter (inheritance included).
                SearchInput {
                    placeholder: t!("resources-label-filter-placeholder").to_string(),
                    value: filter_label,
                    on_submit: move |_| {
                        page.set(0);
                    },
                }
                RefreshButton {
                    on_click: move |_| reload.with_mut(|r| *r += 1),
                }
            }

                ListStates {
                    state: list_state,
                    is_empty: is_empty,
                    empty_message: t!("media-empty-title").to_string(),
                    empty_icon: rsx! {
                        icons::Image { class: "h-8 w-8 text-gray-400" }
                    },
                    empty_detail: rsx! {
                        p { class: "text-gray-600 mt-2", {t!("media-empty-message")} }
                    },
                    div { class: "space-y-4",
                        DataTable {
                            columns: columns,
                            rows: rows,
                            row_key: RowKey::new(|asset: &MediaAsset| asset.id.clone()),
                            on_row_click: Callback::new(move |id: String| selected.set(Some(id))),
                        }
                        ListPager { count: count, page: page }
                    }
                }
            }
        }

            // Upload modal (mounted on demand: fresh state on every open —
            // device_wizard school).
            if upload_open() {
                UploadModal {
                    key: "upload-{upload_open()}",
                    on_close: move |_| upload_open.set(false),
                    on_uploaded: move |_| {
                        upload_open.set(false);
                        reload.with_mut(|r| *r += 1);
                    },
                }
            }

            // Take 360 overlay (mounted on demand: fresh state on every
            // open — device_wizard school; desktop stub is a no-op).
            if take360_open() {
                crate::capture360::overlay::Take360Overlay {
                    key: "take360-{take360_open()}",
                    on_close: move |_| take360_open.set(false),
                    on_uploaded: move |_| {
                        take360_open.set(false);
                        reload.with_mut(|r| *r += 1);
                    },
                }
            }
    }
}

mod detail;
mod preview;
mod upload;
mod util;

use detail::MediaDetail;
use preview::MediaPreview;
use upload::UploadModal;
use util::*;
