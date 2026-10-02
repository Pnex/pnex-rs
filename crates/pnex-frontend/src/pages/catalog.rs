//! Catalogue d'appareils — porté du `Catalog.tsx` React, adapté au contrat
//! paginé Rust (D14) : recherche + filtres type/board **côté serveur**
//! (`search`, `device_type`, `board`), table paginée (socle CRUD —
//! `{count, next, previous, results}`). Each row can open the board
//! pinout preview (`BoardPreviewModal`) when the model's board has a profile.

use dioxus::prelude::*;
use dioxus_i18n::t;
use pnex_core::PredefinedDevice;

use crate::api;
use crate::components::board_pinout_editor::BoardPreviewModal;
use crate::components::crud::filters::{FilterBar, RefreshButton, SearchInput};
use crate::components::crud::layout::ListLayout;
use crate::components::crud::pager::ListPager;
use crate::components::crud::states::ListStates;
use crate::components::crud::table::{Column, DataTable, RowKey};
use crate::components::icons;

/// Taille de page de la liste (3 colonnes × 4 rangées en desktop, héritée
/// de la grille de cartes).
const PAGE_SIZE: i64 = 12;

#[component]
pub fn Catalog() -> Element {
    let mut reload = use_signal(|| 0u32);
    let search = use_signal(String::new);
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
    let boards_resource = use_resource(|| async move { api::boards::list(None).await });
    let mut preview_board = use_signal(|| None::<api::boards::Board>);

    // Lecture synchrone de la ressource (doctrine socle CRUD).
    let (list_state, is_empty, count, rows) = match &*list.value().read() {
        None => (None, false, 0, Vec::new()),
        Some(Ok(paged)) => (
            Some(Ok(())),
            paged.results.is_empty() && paged.count == 0,
            paged.count,
            paged.results.clone(),
        ),
        Some(Err(err)) => (Some(Err(err.clone())), false, 0, Vec::new()),
    };

    // Board profiles (`GET /boards`) backing the "View pinout" button.
    let boards: Vec<api::boards::Board> = match &*boards_resource.read() {
        Some(Ok(list)) => list.clone(),
        _ => Vec::new(),
    };

    // Table columns: device (name + revision), type, board, capabilities
    // (3 + "+n"), pinout preview.
    let columns = vec![
        Column::new(
            t!("catalog-col-device").to_string(),
            |pd: &PredefinedDevice| {
                let pretty = pd.pretty_name.clone().unwrap_or_else(|| pd.name.clone());
                rsx! {
                    div { class: "text-sm font-medium text-gray-900", {pretty} }
                    div { class: "text-xs text-gray-500",
                        {format!("{} · {} {}", pd.name, t!("catalog-rev"), pd.revision)}
                    }
                }
            },
        ),
        Column::new(
            t!("devices-col-type").to_string(),
            |pd: &PredefinedDevice| {
                rsx! { "{pd.device_type}" }
            },
        )
        .with_td_class("text-gray-600"),
        Column::new(
            t!("catalog-col-board").to_string(),
            |pd: &PredefinedDevice| {
                rsx! { {pd.board.clone()} }
            },
        )
        .with_td_class("text-gray-600"),
        Column::new(
            t!("catalog-capabilities").to_string(),
            |pd: &PredefinedDevice| {
                let shown_caps: Vec<String> = pd.capabilities.iter().take(3).cloned().collect();
                let more_caps = pd.capabilities.len().saturating_sub(3);
                rsx! {
                    div { class: "flex flex-wrap gap-1",
                        for cap in shown_caps {
                            span {
                                key: "{cap}",
                                class: "px-2 py-0.5 bg-blue-100 text-blue-800 text-xs rounded-full",
                                {cap}
                            }
                        }
                        if more_caps > 0 {
                            span { class: "px-2 py-0.5 bg-gray-100 text-gray-600 text-xs rounded-full",
                                "+{more_caps}"
                            }
                        }
                    }
                }
            },
        ),
        Column::new(
            t!("catalog-col-pinout").to_string(),
            move |pd: &PredefinedDevice| {
                // Board profile of the model (exact name match); models without a
                // catalog profile (e.g. `generic`) get no button.
                let Some(board) = boards.iter().find(|b| b.name == pd.board).cloned() else {
                    return rsx! {};
                };
                rsx! {
                    button {
                        class: "flex items-center px-3 py-1 text-blue-600 hover:bg-blue-50 rounded-lg transition-colors text-sm",
                        r#type: "button",
                        onclick: move |_| preview_board.set(Some(board.clone())),
                        icons::Eye { class: Some("h-4 w-4 mr-1".into()) }
                        {t!("board-pinout-preview")}
                    }
                }
            },
        ),
    ];

    rsx! {
        ListLayout {
            title: t!("nav-catalog").to_string(),
            subtitle: Some(t!("catalog-subtitle").to_string()),
            can_write: false,
            // Filtres — la carte blanche historique cède la place à la barre
            // canonique du socle (uniformité des pages liste).
            FilterBar {
                SearchInput {
                    placeholder: t!("catalog-search-placeholder").to_string(),
                    value: search,
                    on_submit: move |_| {
                        page.set(0);
                        reload.with_mut(|r| *r += 1);
                    },
                }
                select {
                    class: "px-3 py-2 border border-gray-300 rounded-lg text-sm bg-white",
                    onchange: move |event| {
                        filter_type.set(event.value());
                        page.set(0);
                        reload.with_mut(|r| *r += 1);
                    },
                    option { value: "all", selected: filter_type() == "all", {t!("catalog-type-all")} }
                    option { value: "sensor", selected: filter_type() == "sensor",
                        {t!("devices-type-sensor")}
                    }
                    option {
                        value: "actuator",
                        selected: filter_type() == "actuator",
                        {t!("devices-type-actuator")}
                    }
                    option { value: "mixed", selected: filter_type() == "mixed",
                        {t!("devices-type-mixed")}
                    }
                }
                {
                    match &*options.read() {
                        Some(Ok(models)) => {
                            let mut boards: Vec<String> = models
                                .iter()
                                .map(|pd| pd.board.clone())
                                .collect();
                            boards.sort();
                            boards.dedup();
                            rsx! {
                                select {
                                    class: "px-3 py-2 border border-gray-300 rounded-lg text-sm bg-white",
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
                    }
                }
                RefreshButton { on_click: move |_| reload.with_mut(|r| *r += 1) }
            }

            ListStates {
                state: list_state,
                is_empty,
                empty_message: t!("catalog-empty").to_string(),
                empty_icon: rsx! {
                    icons::Package { class: "h-8 w-8 text-gray-400" }
                },
                empty_detail: rsx! {
                    p { class: "text-gray-400 text-sm mt-2", {t!("catalog-empty-hint")} }
                },
                div { class: "space-y-4",
                    DataTable {
                        columns,
                        rows,
                        row_key: RowKey::new(|pd: &PredefinedDevice| pd.name.clone()),
                    }
                    ListPager { count, page_size: PAGE_SIZE, page }
                }
            }

            if let Some(board) = preview_board() {
                BoardPreviewModal { board, on_close: move |_| preview_board.set(None) }
            }
        }
    }
}
