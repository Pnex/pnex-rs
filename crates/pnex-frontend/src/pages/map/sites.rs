//! Sites: the sites tree without the map (the user's "where" view). Every
//! site (POI) of the org in a full-page list; opening one shows the very
//! drawer of the map — folders, attached objects, preview, labels — so both
//! entry points share one tree and one set of actions.

use super::*;
use crate::components::crud::filters::{FilterBar, LabelFilterInput, SearchInput};
use crate::components::crud::layout::ListLayout;
use crate::components::crud::pager::{ListPager, PAGE_SIZE};
use crate::components::crud::states::ListStates;
use crate::components::labels_editor::use_row_labels;
use crate::components::ui::BTN_SECONDARY;

#[component]
pub fn Sites() -> Element {
    let search = use_signal(String::new);
    // D218: own label filter ("name" or "name:value").
    let filter_label = use_signal(String::new);
    let mut reload = use_signal(|| 0u32);
    let mut site_page = use_signal(|| 0i64);
    let mut selected = use_signal(|| None::<String>);
    let mut preview = use_signal(|| None::<PreviewTarget>);
    let mut pinned = use_signal(|| None::<PreviewTarget>);
    let mut creating = use_signal(|| false);
    let navigator = use_navigator();
    let can_write = crate::state::org::current_can_write();

    // Deep link (location breadcrumb, global search): consume the requested
    // site once (the guard keeps the effect from re-arming).
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

    let list = use_resource(move || async move {
        reload();
        let filters = viz::PoiFilters {
            search: Some(search()),
            label: Some(filter_label().trim().to_string()),
            limit: Some(PAGE_SIZE),
            offset: Some(site_page() * PAGE_SIZE),
            ..Default::default()
        };
        viz::list_pois(&filters).await
    });
    // D218: own labels of the page rows, one batch per list (re)load.
    let row_labels = use_row_labels(pnex_core::resources::KIND_MAP_PIN, move || {
        match &*list.read() {
            Some(Ok(p)) => p.results.iter().map(|s| s.id.clone()).collect(),
            _ => Vec::new(),
        }
    });
    // An API error used to show the empty list; ListStates now surfaces it.
    let (state, sites, total) = match &*list.read() {
        None => (None, Vec::new(), 0),
        Some(Ok(p)) => (Some(Ok(())), p.results.clone(), p.count),
        Some(Err(err)) => (Some(Err(err.clone())), Vec::new(), 0),
    };

    rsx! {
        ListLayout {
            title: t!("sites-title").to_string(),
            subtitle: Some(t!("sites-subtitle").to_string()),
            add_label: Some(t!("sites-add").to_string()),
            on_add: move |_| creating.set(true),
            on_refresh: move |_| reload += 1,
            can_write,
            actions: rsx! {
                button {
                    class: BTN_SECONDARY,
                    r#type: "button",
                    onclick: move |_| {
                        navigator.push(Route::Map {});
                    },
                    icons::Map { class: "h-4 w-4" }
                    {t!("sites-open-map")}
                }
            },
            filters: rsx! {
                FilterBar {
                    SearchInput {
                        placeholder: t!("poi-search-placeholder").to_string(),
                        value: search,
                        on_submit: move |_| {
                            site_page.set(0);
                            reload += 1;
                        },
                    }
                    LabelFilterInput {
                        placeholder: t!("resources-label-filter-placeholder").to_string(),
                        value: filter_label,
                        on_submit: move |_| {
                            site_page.set(0);
                            reload += 1;
                        },
                    }
                }
            },
            ListStates {
                state,
                is_empty: total == 0,
                empty_message: t!("sites-empty").to_string(),
                div { class: "bg-white rounded-lg border border-line overflow-hidden",
                    for site in sites.iter() {
                        PoiRow {
                            key: "{site.id}",
                            poi: site.clone(),
                            positions: Vec::new(),
                            labels: row_labels.get(&site.id).cloned().unwrap_or_default(),
                            on_select: move |id: String| selected.set(Some(id)),
                        }
                    }
                    div { class: "px-4 py-2 text-xs text-gray-500", {t!("poi-count", count : total)} }
                }
                ListPager { count: total, page: site_page }
            }
        }
        // Same drawer as the map: tree, attached objects, preview, labels.
        if let Some(id) = selected() {
            PoiDetail {
                key: "{id}",
                poi_id: id,
                preview,
                pinned,
                on_close: move |_| {
                    selected.set(None);
                    preview.set(None);
                    pinned.set(None);
                },
                on_changed: move |_| reload += 1,
            }
        }
        if creating() {
            // A site without a map click starts on the default centre; the
            // form shows the coordinates, which can be typed.
            PoiFormModal {
                initial: None,
                coords: (DEFAULT_CENTER.1, DEFAULT_CENTER.0),
                on_saved: move |_| {
                    creating.set(false);
                    reload += 1;
                },
                on_close: move |_| creating.set(false),
            }
        }
    }
}
