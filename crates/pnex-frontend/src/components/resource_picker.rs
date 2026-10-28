//! Generic picker of objects attachable to a POI (map drawer) —
//! generalisation of `tour_editor/media_picker.rs` (Modal + search +
//! Pager). The picker **mutates nothing**: it emits a [`ResourcePick`] and the
//! owner performs the mutation (POST edge D42 for media/tour/dashboard,
//! POST `/pois/{id}/devices` for a device — D43: several devices per
//! POI, one placement per device, move confirmed if already placed).

use dioxus::prelude::*;
use dioxus_i18n::t;

use crate::api;
use crate::api::error::ApiError;
use crate::components::gerbe::Gerbe;
use crate::components::modal::Modal;
use crate::components::pager::Pager;
use crate::components::ui::INPUT;

const PAGE_SIZE: i64 = 8;

/// Choice coming out of the picker — the owner performs the mutation.
#[derive(Clone, Debug)]
pub enum ResourcePick {
    MediaAsset { id: String, name: String },
    Tour { id: String, name: String },
    Dashboard { id: String, name: String },
    Device { slug: String },
}

/// Picker tabs (target kinds of `placed_on` — D42 registry).
#[derive(Clone, Copy, PartialEq)]
pub enum PickerTab {
    Media,
    Tour,
    Dashboard,
    Device,
}

/// Normalised row, all kinds together.
#[derive(Clone)]
struct PickerRow {
    id: String,
    name: String,
    subtitle: String,
}

#[derive(Clone)]
struct PickerPage {
    count: i64,
    rows: Vec<PickerRow>,
}

/// Modal for choosing an object to attach. Tabs Media / 3D tour /
/// Dashboard / Device, search + D14 pagination (real count).
#[component]
pub fn ResourcePicker(
    initial_tab: Option<PickerTab>,
    /// Media tab only (an object's gallery, D231).
    #[props(default)]
    media_only: bool,
    on_picked: Callback<ResourcePick>,
    on_close: Callback<()>,
) -> Element {
    let mut tab = use_signal(|| initial_tab.unwrap_or(PickerTab::Media));
    let mut search = use_signal(String::new);
    let mut page = use_signal(|| 0i64);

    // A single resource for the 4 tabs: the signals are read INSIDE the
    // closure (dioxus trap #2784 — never a captured frozen value), and the
    // result is normalised into PickerPage (real count → faithful Pager).
    let list = use_resource(move || {
        let search = {
            let value = search().trim().to_string();
            (!value.is_empty()).then_some(value)
        };
        let offset = page() * PAGE_SIZE;
        async move {
            let page: Result<PickerPage, ApiError> = match tab() {
                PickerTab::Media => {
                    // All kinds (panorama, photo, splat, floorplan) —
                    // empty `kinds` = all on the client side.
                    api::media::list(&api::media::MediaFilters {
                        kinds: vec![],
                        search,
                        limit: Some(PAGE_SIZE),
                        offset: Some(offset),
                        ..Default::default()
                    })
                    .await
                    .map(|p| PickerPage {
                        count: p.count,
                        rows: p
                            .results
                            .into_iter()
                            .map(|a| {
                                // Subtitle computed BEFORE the move of the
                                // fields (media_kind reads `a`).
                                let subtitle =
                                    format!("{:?} · v{}", a.media_kind(), a.latest_version_number);
                                PickerRow {
                                    id: a.id,
                                    name: a.name,
                                    subtitle,
                                }
                            })
                            .collect(),
                    })
                }
                PickerTab::Tour => api::tours::list(&api::tours::TourFilters {
                    search,
                    limit: Some(PAGE_SIZE),
                    offset: Some(offset),
                    ..Default::default()
                })
                .await
                .map(|p| PickerPage {
                    count: p.count,
                    rows: p
                        .results
                        .into_iter()
                        .map(|t| PickerRow {
                            id: t.id,
                            name: t.name,
                            subtitle: t.mode,
                        })
                        .collect(),
                }),
                PickerTab::Dashboard => api::dashboards::list(&api::dashboards::DashboardFilters {
                    search,
                    limit: Some(PAGE_SIZE),
                    offset: Some(offset),
                    ..Default::default()
                })
                .await
                .map(|p| PickerPage {
                    count: p.count,
                    rows: p
                        .results
                        .into_iter()
                        .map(|d| PickerRow {
                            id: d.id,
                            name: d.name,
                            subtitle: format!("v{}", d.current_version_number),
                        })
                        .collect(),
                }),
                PickerTab::Device => api::devices::list(&api::devices::DeviceFilters {
                    search,
                    limit: Some(PAGE_SIZE),
                    offset: Some(offset),
                    ..Default::default()
                })
                .await
                .map(|p| PickerPage {
                    count: p.count,
                    rows: p
                        .results
                        .into_iter()
                        .map(|d| PickerRow {
                            id: d.device_id.clone(),
                            name: d.device_id,
                            subtitle: d.device_type,
                        })
                        .collect(),
                }),
            };
            page
        }
    });

    rsx! {
        Modal {
            title: t!("poi-picker-title").to_string(),
            max_width: "max-w-lg".to_string(),
            on_close,
            div { class: "space-y-3",
                // Tabs (blue active pill) — const array (dioxus: no
                // tuple destructuring in the rsx `for`).
                div { class: "flex flex-wrap gap-1", hidden: media_only,
                    for tab_def in [
                        (PickerTab::Media, "poi-picker-tab-media"),
                        (PickerTab::Tour, "poi-picker-tab-tour"),
                        (PickerTab::Dashboard, "poi-picker-tab-dashboard"),
                        (PickerTab::Device, "poi-picker-tab-device"),
                    ]
                    {
                        button {
                            key: "{tab_def.1}",
                            class: if tab() == tab_def.0 { "px-3 py-1.5 text-sm font-medium text-signal-700 bg-signal-50 rounded-full" } else { "px-3 py-1.5 text-sm text-gray-600 hover:bg-gray-100 rounded-full transition-colors" },
                            onclick: move |_| {
                                tab.set(tab_def.0);
                                page.set(0);
                            },
                            {t!(tab_def.1)}
                        }
                    }
                }
                input {
                    class: "{INPUT} w-full",
                    r#type: "search",
                    placeholder: t!("poi-picker-search"),
                    value: "{search}",
                    oninput: move |event| {
                        search.set(event.value());
                        page.set(0);
                    },
                }
                // D43: a device has only one placement — attaching it here while
                // it is placed elsewhere opens a confirmation (409).
                if tab() == PickerTab::Device {
                    p { class: "px-3 py-2 text-xs text-amber-700 bg-amber-50 border border-amber-200 rounded-lg",
                        {t!("poi-picker-device-hint")}
                    }
                }
                match &*list.value().read() {
                    Some(Ok(result)) if result.rows.is_empty() => rsx! {
                        p { class: "text-sm text-gray-500 text-center py-6", {t!("poi-picker-empty")} }
                    },
                    Some(Ok(result)) => rsx! {
                        ul { class: "divide-y divide-gray-100 max-h-80 overflow-y-auto",
                            for row in result.rows.clone() {
                                li { key: "{row.id}",
                                    button {
                                        class: "w-full text-left px-3 py-2 hover:bg-signal-50 transition-colors rounded-lg",
                                        onclick: move |_| {
                                            // Current tab READ on click
                                            // (the row is rendered under
                                            // the active tab).
                                            let pick = match tab() {
                                                PickerTab::Media => {
                                                    ResourcePick::MediaAsset {
                                                        id: row.id.clone(),
                                                        name: row.name.clone(),
                                                    }
                                                }
                                                PickerTab::Tour => {
                                                    ResourcePick::Tour {
                                                        id: row.id.clone(),
                                                        name: row.name.clone(),
                                                    }
                                                }
                                                PickerTab::Dashboard => {
                                                    ResourcePick::Dashboard {
                                                        id: row.id.clone(),
                                                        name: row.name.clone(),
                                                    }
                                                }
                                                PickerTab::Device => {
                                                    ResourcePick::Device {
                                                        slug: row.id.clone(),
                                                    }
                                                }
                                            };
                                            on_picked.call(pick);
                                        },
                                        div { class: "text-sm font-medium text-gray-900", {row.name.clone()} }
                                        div { class: "text-xs text-gray-500", {row.subtitle.clone()} }
                                    }
                                }
                            }
                        }
                        Pager {
                            count: result.count,
                            page_size: PAGE_SIZE,
                            page,
                            on_navigate: move |new_page| page.set(new_page),
                        }
                    },
                    Some(Err(err)) => rsx! {
                        div { class: "bg-red-50 border border-red-200 rounded-lg p-3 text-sm text-red-700",
                            {err.message.clone()}
                        }
                    },
                    None => rsx! {
                        div { class: "flex justify-center py-6",
                            Gerbe { size: 28 }
                        }
                    },
                }
            }
        }
    }
}
