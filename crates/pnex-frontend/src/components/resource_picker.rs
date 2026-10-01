//! Sélecteur générique d'objets attachables à un POI (map drawer) —
//! généralisation de `tour_editor/media_picker.rs` (Modal + recherche +
//! Pager). Le picker **ne mute rien** : il émet un [`ResourcePick`] et le
//! détenteur exécute la mutation (POST edge D42 pour média/tour/dashboard,
//! POST `/pois/{id}/devices` pour un device — D43 : plusieurs devices par
//! POI, un placement par device, déplacement confirmé s'il est déjà placé).

use dioxus::prelude::*;
use dioxus_i18n::t;

use crate::api;
use crate::api::error::ApiError;
use crate::components::modal::Modal;
use crate::components::pager::Pager;

const PAGE_SIZE: i64 = 8;

/// Choix sorti du picker — le détenteur exécute la mutation.
#[derive(Clone, Debug)]
pub enum ResourcePick {
    MediaAsset { id: String, name: String },
    Tour { id: String, name: String },
    Dashboard { id: String, name: String },
    Device { slug: String },
}

/// Onglets du picker (kinds cibles de `placed_on` — registre D42).
#[derive(Clone, Copy, PartialEq)]
pub enum PickerTab {
    Media,
    Tour,
    Dashboard,
    Device,
}

/// Ligne normalisée, tous kinds confondus.
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

/// Modal de choix d'un objet à attacher. Onglets Média / Parcours 3D /
/// Tableau de bord / Device, recherche + pagination D14 (count réel).
#[component]
pub fn ResourcePicker(
    initial_tab: Option<PickerTab>,
    on_picked: Callback<ResourcePick>,
    on_close: Callback<()>,
) -> Element {
    let mut tab = use_signal(|| initial_tab.unwrap_or(PickerTab::Media));
    let mut search = use_signal(String::new);
    let mut page = use_signal(|| 0i64);

    // Une seule resource pour les 4 onglets : les signaux sont lus DANS la
    // closure (piège dioxus #2784 — jamais de valeur figée capturée), et le
    // résultat est normalisé en PickerPage (count réel → Pager fidèle).
    let list = use_resource(move || {
        let search = {
            let value = search().trim().to_string();
            (!value.is_empty()).then_some(value)
        };
        let offset = page() * PAGE_SIZE;
        async move {
            let page: Result<PickerPage, ApiError> = match tab() {
                PickerTab::Media => {
                    // Tous kinds (panorama, photo, splat, floorplan) —
                    // `kinds` vide = tous côté client.
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
                                // Sous-titre calculé AVANT le move des
                                // champs (media_kind lit `a`).
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
                // Onglets (pill actif bleu) — tableau const (dioxus : pas de
                // déstructuration de tuple dans le `for` du rsx).
                div { class: "flex flex-wrap gap-1",
                    for tab_def in [
                        (PickerTab::Media, "poi-picker-tab-media"),
                        (PickerTab::Tour, "poi-picker-tab-tour"),
                        (PickerTab::Dashboard, "poi-picker-tab-dashboard"),
                        (PickerTab::Device, "poi-picker-tab-device"),
                    ] {
                        button {
                            key: "{tab_def.1}",
                            class: if tab() == tab_def.0 { "px-3 py-1.5 text-sm font-medium text-blue-700 bg-blue-50 rounded-full" } else { "px-3 py-1.5 text-sm text-gray-600 hover:bg-gray-100 rounded-full transition-colors" },
                            onclick: move |_| {
                                tab.set(tab_def.0);
                                page.set(0);
                            },
                            {t!(tab_def.1)}
                        }
                    }
                }
                input {
                    class: "w-full px-3 py-2 border border-gray-300 rounded-lg text-sm",
                    r#type: "search",
                    placeholder: t!("poi-picker-search"),
                    value: "{search}",
                    oninput: move |event| {
                        search.set(event.value());
                        page.set(0);
                    },
                }
                // D43 : un device n'a qu'un placement — l'attacher ici alors
                // qu'il est placé ailleurs ouvre une confirmation (409).
                if tab() == PickerTab::Device {
                    p { class: "px-3 py-2 text-xs text-amber-700 bg-amber-50 border border-amber-200 rounded-lg",
                        {t!("poi-picker-device-hint")}
                    }
                }
                match &*list.value().read() {
                    Some(Ok(result)) if result.rows.is_empty() => rsx! {
                        p { class: "text-sm text-gray-400 text-center py-6",
                            {t!("poi-picker-empty")}
                        }
                    },
                    Some(Ok(result)) => rsx! {
                        ul { class: "divide-y divide-gray-100 max-h-80 overflow-y-auto",
                            for row in result.rows.clone() {
                                li { key: "{row.id}",
                                    button {
                                        class: "w-full text-left px-3 py-2 hover:bg-blue-50 transition-colors rounded-lg",
                                        onclick: move |_| {
                                            // Onglet courant LU au clic
                                            // (la ligne est rendue sous
                                            // l'onglet actif).
                                            let pick = match tab() {
                                                PickerTab::Media => ResourcePick::MediaAsset {
                                                    id: row.id.clone(),
                                                    name: row.name.clone(),
                                                },
                                                PickerTab::Tour => ResourcePick::Tour {
                                                    id: row.id.clone(),
                                                    name: row.name.clone(),
                                                },
                                                PickerTab::Dashboard => ResourcePick::Dashboard {
                                                    id: row.id.clone(),
                                                    name: row.name.clone(),
                                                },
                                                PickerTab::Device => ResourcePick::Device {
                                                    slug: row.id.clone(),
                                                },
                                            };
                                            on_picked.call(pick);
                                        },
                                        div { class: "text-sm font-medium text-gray-900", {row.name.clone()} }
                                        div { class: "text-xs text-gray-400", {row.subtitle.clone()} }
                                    }
                                }
                            }
                        }
                        Pager {
                            count: result.count,
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
            }
        }
    }
}
