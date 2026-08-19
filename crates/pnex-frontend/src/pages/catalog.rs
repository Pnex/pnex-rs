//! Catalogue d'appareils — porté du `Catalog.tsx` React, adapté au contrat
//! paginé Rust (D14) : recherche + filtres type/board **côté serveur**
//! (`search`, `device_type`, `board`), grille de cartes paginée
//! (`{count, next, previous, results}`). Les actions d'achat/docs pointent
//! vers les URLs Prestashop/BYOD du modèle ; « Configurer » mène au flux
//! d'enregistrement de la page Devices (le build firmware arrive Phase 6).

use dioxus::prelude::*;
use dioxus_i18n::t;

use crate::api;
use crate::components::icons;
use crate::components::pager::Pager;

/// Taille de page de la grille (3 colonnes × 4 rangées en desktop).
const PAGE_SIZE: i64 = 12;

#[component]
pub fn Catalog() -> Element {
    let mut reload = use_signal(|| 0u32);
    let mut search = use_signal(String::new);
    let mut filter_type = use_signal(|| "all".to_string());
    let mut filter_board = use_signal(|| "all".to_string());
    let mut page = use_signal(|| 0i64);

    // Page courante de la grille — filtres poussés au serveur.
    let list = use_resource(move || {
        let filters = api::devices::CatalogFilters {
            search: {
                let value = search().trim().to_string();
                if value.is_empty() {
                    None
                } else {
                    Some(value)
                }
            },
            device_type: match filter_type().as_str() {
                "all" => None,
                other => Some(other.to_string()),
            },
            board: match filter_board().as_str() {
                "all" => None,
                other => Some(other.to_string()),
            },
            limit: PAGE_SIZE,
            offset: page() * PAGE_SIZE,
        };
        async move {
            let _ = reload();
            api::devices::predefined_devices_page(&filters).await
        }
    });

    // Options des filtres (vocabulaire type + boards distincts) — une seule
    // requête page max : le catalogue de référence est borné.
    let options = use_resource(|| async move { api::devices::predefined_devices().await });

    rsx! {
        div { class: "p-6",
            div { class: "mb-8",
                h1 { class: "text-3xl font-bold text-gray-900", {t!("nav-catalog")} }
                p { class: "text-gray-600 mt-2", {t!("catalog-subtitle")} }
            }

            // Filtres
            div { class: "bg-white rounded-lg shadow-sm p-6 mb-8 flex flex-wrap gap-4 items-center",
                input {
                    class: "flex-1 min-w-64 px-3 py-2 border border-gray-300 rounded-lg text-sm",
                    r#type: "search",
                    placeholder: t!("catalog-search-placeholder"),
                    value: "{search}",
                    oninput: move |event| search.set(event.value()),
                    onkeydown: move |event| {
                        if event.key() == Key::Enter {
                            event.prevent_default();
                            page.set(0);
                            reload.with_mut(|r| *r += 1);
                        }
                    },
                }
                select {
                    class: "min-w-40 px-3 py-2 border border-gray-300 rounded-lg text-sm bg-white",
                    onchange: move |event| {
                        filter_type.set(event.value());
                        page.set(0);
                        reload.with_mut(|r| *r += 1);
                    },
                    option { value: "all", selected: filter_type() == "all", {t!("catalog-type-all")} }
                    option { value: "sensor", selected: filter_type() == "sensor", {t!("devices-type-sensor")} }
                    option { value: "actuator", selected: filter_type() == "actuator", {t!("devices-type-actuator")} }
                    option { value: "mixed", selected: filter_type() == "mixed", {t!("devices-type-mixed")} }
                }
                {match &*options.read() {
                    Some(Ok(models)) => {
                        let mut boards: Vec<String> =
                            models.iter().map(|pd| pd.board.clone()).collect();
                        boards.sort();
                        boards.dedup();
                        rsx! {
                            select {
                                class: "min-w-40 px-3 py-2 border border-gray-300 rounded-lg text-sm bg-white",
                                onchange: move |event| {
                                    filter_board.set(event.value());
                                    page.set(0);
                                    reload.with_mut(|r| *r += 1);
                                },
                                option { value: "all", selected: filter_board() == "all", {t!("catalog-board-all")} }
                                for board in boards {
                                    option {
                                        key: "{board}",
                                        value: "{board}",
                                        selected: filter_board() == board,
                                        {board.clone()}
                                    }
                                }
                            }
                        }
                    }
                    _ => rsx! {},
                }}
                button {
                    class: "px-3 py-2 text-sm text-gray-600 border border-gray-300 rounded-lg hover:bg-gray-50 transition-colors",
                    onclick: move |_| reload.with_mut(|r| *r += 1),
                    icons::RefreshCw { class: "h-4 w-4 inline mr-1" }
                    {t!("common-refresh")}
                }
            }

            match &*list.value().read() {
                Some(Ok(paged)) if paged.results.is_empty() && paged.count == 0 => rsx! {
                    div { class: "text-center py-12",
                        div { class: "p-4 bg-gray-100 rounded-full w-16 h-16 mx-auto mb-4",
                            icons::Package { class: "h-8 w-8 text-gray-400 mx-auto mt-2" }
                        }
                        p { class: "text-gray-500 text-lg", {t!("catalog-empty")} }
                        p { class: "text-gray-400 text-sm mt-2", {t!("catalog-empty-hint")} }
                    }
                },
                Some(Ok(paged)) => rsx! {
                    div { class: "grid grid-cols-1 md:grid-cols-2 lg:grid-cols-3 gap-6",
                        for pd in paged.results.clone() {
                            {catalog_card(pd)}
                        }
                    }
                    Pager {
                        count: paged.count,
                        page_size: PAGE_SIZE,
                        page: page,
                        on_navigate: move |new_page| {
                            page.set(new_page);
                        },
                    }
                },
                Some(Err(err)) => rsx! {
                    div { class: "bg-red-50 border border-red-200 rounded-lg p-4 text-sm text-red-700", {err.message.clone()} }
                },
                None => rsx! {
                    div { class: "text-center py-12",
                        span { class: "animate-spin inline-block rounded-full h-8 w-8 border-b-2 border-blue-600" }
                    }
                },
            }
        }
    }
}

/// Carte d'un appareil du catalogue — image, identité, capacités, liens.
fn catalog_card(pd: pnex_core::PredefinedDevice) -> Element {
    let pretty = pd.pretty_name.clone().unwrap_or_else(|| pd.name.clone());
    let shown_caps: Vec<String> = pd.capabilities.iter().take(4).cloned().collect();
    let more_caps = pd.capabilities.len().saturating_sub(4);
    rsx! {
        div { class: "bg-white rounded-lg shadow-sm border border-gray-200 overflow-hidden hover:shadow-md transition-shadow flex flex-col",
            div { class: "aspect-video bg-gray-100 flex items-center justify-center",
                match pd.image_source_url.as_deref() {
                    Some(url) => rsx! {
                        img {
                            class: "w-full h-full object-cover",
                            src: url,
                            alt: "{pretty}",
                        }
                    },
                    None => rsx! {
                        div { class: "p-8",
                            div { class: "w-16 h-16 mx-auto mb-2 bg-gray-200 rounded-lg flex items-center justify-center",
                                icons::Package { class: "h-8 w-8 text-gray-400" }
                            }
                            p { class: "text-sm text-gray-500 text-center", {t!("catalog-no-image")} }
                        }
                    },
                }
            }

            div { class: "p-6 flex-1 flex flex-col",
                div { class: "mb-4",
                    h3 { class: "text-lg font-semibold text-gray-900 mb-1", {pretty.clone()} }
                    p { class: "text-sm text-gray-600", "{pd.device_type} • {pd.board}" }
                    p { class: "text-sm text-gray-500 mt-1", "{t!(\"catalog-rev\")} {pd.revision}" }
                }

                if let Some(description) = pd.description.as_deref() {
                    p { class: "text-sm text-gray-700 mb-4 line-clamp-3", {description} }
                }

                div { class: "mb-4",
                    h4 { class: "text-sm font-medium text-gray-900 mb-2", {t!("catalog-capabilities")} }
                    div { class: "flex flex-wrap gap-1",
                        for cap in shown_caps {
                            span {
                                key: "{cap}",
                                class: "px-2 py-1 bg-blue-100 text-blue-800 text-xs rounded-full",
                                {cap.clone()}
                            }
                        }
                        if more_caps > 0 {
                            span { class: "px-2 py-1 bg-gray-100 text-gray-600 text-xs rounded-full",
                                "+{more_caps}"
                            }
                        }
                    }
                }

                div { class: "mt-auto flex items-center justify-between pt-4 border-t border-gray-100",
                    div { class: "flex items-center gap-2",
                        if let Some(url) = pd.byod_doc_url.as_deref() {
                            a {
                                class: "flex items-center px-3 py-1 text-blue-600 hover:bg-blue-50 rounded-lg transition-colors",
                                href: url,
                                target: "_blank",
                                rel: "noopener noreferrer",
                                icons::BookOpen { class: "h-4 w-4 mr-1" }
                                {t!("catalog-docs")}
                            }
                        }
                        if let Some(url) = pd.prestashop_buy_url.as_deref() {
                            a {
                                class: "flex items-center px-3 py-1 text-green-600 hover:bg-green-50 rounded-lg transition-colors",
                                href: url,
                                target: "_blank",
                                rel: "noopener noreferrer",
                                icons::ShoppingCart { class: "h-4 w-4 mr-1" }
                                {t!("catalog-buy")}
                            }
                        }
                    }
                    Link {
                        to: crate::app::Route::Devices {},
                        class: "flex items-center px-3 py-1 bg-blue-600 text-white rounded-lg hover:bg-blue-700 transition-colors text-sm",
                        {t!("catalog-configure")}
                    }
                }
            }
        }
    }
}
