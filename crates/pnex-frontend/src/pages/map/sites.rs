//! Sites: the sites tree without the map (the user's "where" view). Every
//! site (POI) of the org in a full-page list; opening one shows the very
//! drawer of the map — folders, attached objects, preview, labels — so both
//! entry points share one tree and one set of actions.

use super::*;
use crate::components::crud::filters::{FilterBar, SearchInput};
use crate::components::crud::layout::ListLayout;

#[component]
pub fn Sites() -> Element {
    let search = use_signal(String::new);
    let mut reload = use_signal(|| 0u32);
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
            ..Default::default()
        };
        viz::list_pois(&filters).await
    });
    let loading = list.read().is_none();
    let page = list.read().clone().and_then(Result::ok);
    let sites = page.as_ref().map(|p| p.results.clone()).unwrap_or_default();
    let total = page.as_ref().map(|p| p.count).unwrap_or(0);

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
                    class: "inline-flex items-center gap-2 px-4 py-2 text-sm font-medium text-gray-700 bg-white border border-gray-300 rounded-lg hover:bg-gray-50 transition-colors",
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
                        on_submit: move |_| reload += 1,
                    }
                }
            },
            div { class: "bg-white rounded-lg shadow-sm overflow-hidden",
                if loading {
                    div { class: "p-6 flex justify-center",
                        span { class: "animate-spin rounded-full h-5 w-5 border-b-2 border-blue-600" }
                    }
                } else if sites.is_empty() {
                    div { class: "p-8 text-center text-sm text-gray-500", {t!("sites-empty")} }
                } else {
                    for site in sites.iter() {
                        PoiRow {
                            key: "{site.id}",
                            poi: site.clone(),
                            positions: Vec::new(),
                            on_select: move |id: String| selected.set(Some(id)),
                        }
                    }
                    div { class: "px-4 py-2 text-xs text-gray-500", {t!("poi-count", count : total)} }
                }
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
