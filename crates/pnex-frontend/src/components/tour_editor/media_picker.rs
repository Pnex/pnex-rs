//! Media asset picker (modal) — list filtered by kind (approach of the
//! media page's `UploadModal`/`AttachPlan`): search + Pager, single
//! choice. No upload here (the plan/panorama is prepared in the Media
//! page) — V1.

use dioxus::prelude::*;
use dioxus_i18n::t;

use crate::api;
use crate::api::media::{MediaFilters, MediaKind, MediaPage};
use crate::components::gerbe::Gerbe;
use crate::components::modal::Modal;
use crate::components::pager::Pager;
use crate::components::ui::INPUT;

const PAGE_SIZE: i64 = 8;

/// Modal to choose a media asset. `kind` = semantic use (label +
/// filtered list — the floor plan also accepts `photo`); `on_picked`
/// on choice. No upload here: the Media page prepares the files.
#[component]
pub(crate) fn MediaPicker(
    kind: MediaKind,
    on_picked: Callback<(String, String)>,
    on_close: Callback<()>,
) -> Element {
    let mut search = use_signal(String::new);
    let mut page = use_signal(|| 0i64);

    let kind_label = match kind {
        MediaKind::Panorama => t!("media-kind-panorama"),
        MediaKind::Floorplan => t!("media-kind-floorplan"),
        _ => t!("media-kind-photo"),
    };

    // A floor plan does not require the `floorplan` kind: any
    // image displays (photo included) — we list both kinds. The other
    // uses stay strict (scene = panorama, otherwise broken rendering).
    let kinds: Vec<MediaKind> = match kind {
        MediaKind::Floorplan => vec![MediaKind::Floorplan, MediaKind::Photo],
        other => vec![other],
    };

    let list = use_resource(move || {
        let filters = MediaFilters {
            label: None,
            kinds: kinds.clone(),
            search: {
                let value = search().trim().to_string();
                (!value.is_empty()).then_some(value)
            },
            limit: Some(PAGE_SIZE),
            offset: Some(page() * PAGE_SIZE),
        };
        async move {
            api::media::list(&filters)
                .await
                .map(|MediaPage { results, .. }| results)
        }
    });

    rsx! {
        Modal {
            title: t!("studio-picker-title", kind : kind_label),
            max_width: "max-w-lg".to_string(),
            on_close,
            div { class: "space-y-3",
                input {
                    class: "{INPUT} w-full",
                    r#type: "search",
                    placeholder: t!("studio-picker-search"),
                    value: "{search}",
                    oninput: move |event| {
                        search.set(event.value());
                        page.set(0);
                    },
                }
                match &*list.value().read() {
                    Some(Ok(items)) if items.is_empty() => rsx! {
                        p { class: "text-sm text-gray-500 text-center py-6", {t!("studio-picker-empty")} }
                    },
                    Some(Ok(items)) => rsx! {
                        ul { class: "divide-y divide-gray-100 max-h-80 overflow-y-auto",
                            for asset in items.clone() {
                                li { key: "{asset.id}",
                                    button {
                                        class: "w-full text-left px-3 py-2 hover:bg-signal-50 transition-colors rounded-lg",
                                        onclick: move |_| {
                                            on_picked.call((asset.id.clone(), asset.name.clone()));
                                        },
                                        div { class: "text-sm font-medium text-gray-900", {asset.name.clone()} }
                                        div { class: "text-xs text-gray-500",
                                            {format!("{} · v{}", asset.id, asset.latest_version_number)}
                                        }
                                    }
                                }
                            }
                        }
                        // The count comes from a separate resource: we paginate
                        // on the current page size (the Picker counts everything).
                        Pager {
                            count: items.len() as i64,
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
                p { class: "text-xs text-gray-500", {t!("studio-picker-hint")} }
            }
        }
    }
}
