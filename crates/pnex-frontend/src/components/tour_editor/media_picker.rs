//! Sélecteur d'asset média (modal) — liste filtrée par kind (école
//! `UploadModal`/`AttachPlan` de la page média) : recherche + Pager, choix
//! unique. Pas d'upload ici (le plan/panorama se prépare dans la page
//! Média) — V1.

use dioxus::prelude::*;
use dioxus_i18n::t;

use crate::api;
use crate::api::media::{MediaFilters, MediaKind, MediaPage};
use crate::components::modal::Modal;
use crate::components::pager::Pager;

const PAGE_SIZE: i64 = 8;

/// Modal de choix d'un asset média. `kind` = usage sémantique (étiquette +
/// liste filtrée — le plan d'étage admet aussi les `photo`) ; `on_picked`
/// au choix. Pas d'upload ici : la page Média prépare les fichiers.
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

    // Un plan d'étage n'exige pas le kind `floorplan` : n'importe quelle
    // image s'affiche (photo incluse) — on liste les deux kinds. Les autres
    // usages restent stricts (scène = panorama, sinon rendu cassé).
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
            title: t!("studio-picker-title", kind: kind_label),
            max_width: "max-w-lg".to_string(),
            on_close,
            div { class: "space-y-3",
                input {
                    class: "w-full px-3 py-2 border border-gray-300 rounded-lg text-sm",
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
                        p { class: "text-sm text-gray-400 text-center py-6",
                            {t!("studio-picker-empty")}
                        }
                    },
                    Some(Ok(items)) => rsx! {
                        ul { class: "divide-y divide-gray-100 max-h-80 overflow-y-auto",
                            for asset in items.clone() {
                                li { key: "{asset.id}",
                                    button {
                                        class: "w-full text-left px-3 py-2 hover:bg-blue-50 transition-colors rounded-lg",
                                        onclick: move |_| {
                                            on_picked.call((asset.id.clone(), asset.name.clone()));
                                        },
                                        div { class: "text-sm font-medium text-gray-900", {asset.name.clone()} }
                                        div { class: "text-xs text-gray-400",
                                            {format!("{} · v{}", asset.id, asset.latest_version_number)}
                                        }
                                    }
                                }
                            }
                        }
                        // Le compte vient d'une resource séparée : on paginate
                        // sur la taille de page courante (le Picker compte tout).
                        Pager {
                            count: items.len() as i64,
                            page_size: PAGE_SIZE,
                            page: page,
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
                            span { class: "animate-spin rounded-full h-6 w-6 border-b-2 border-blue-600" }
                        }
                    },
                }
                p { class: "text-xs text-gray-400",
                    {t!("studio-picker-hint")}
                }
            }
        }
    }
}
