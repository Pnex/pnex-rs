//! Studio (parcours 3D) — liste des tours de l'org et éditeur
//! (`components/tour_editor/`, sous-vue pilotée par le signal local
//! `selected` — pattern `flows.rs`).
//!
//! Le scoping org vient du client (`X-Org-Id`), l'écriture est réservée
//! owner/admin (le serveur force, l'UI masque).

use dioxus::prelude::*;
use dioxus_i18n::t;
use pnex_core::TourSummary;

use crate::api;
use crate::components::badges::date_label;
use crate::components::confirm::ConfirmDialog;
use crate::components::crud::filters::{FilterBar, SearchInput};
use crate::components::crud::layout::ListLayout;
use crate::components::crud::pager::{ListPager, PAGE_SIZE};
use crate::components::crud::states::ListStates;
use crate::components::crud::table::{Column, DataTable, RowKey};
use crate::components::icons;
use crate::state::tours::OPEN_TOUR;
use crate::state::{org, session, toasts};

/// Rôle de l'utilisateur dans l'org courante — même helper que `flows.rs`
/// (privé par page, convention projet).
fn current_role() -> Option<String> {
    let user = session::user()?;
    let org_id = org::current()?;
    user.orgs
        .iter()
        .find(|m| m.id == org_id)
        .map(|m| m.role.clone())
}

#[component]
pub fn Studio() -> Element {
    let mut reload = use_signal(|| 0u32);
    let mut selected = use_signal(|| None::<String>);

    // Deep-link (one-shot signal, consumed once).
    let mut deep_link_done = use_signal(|| false);
    use_effect(move || {
        if deep_link_done() {
            return;
        }
        if let Some(tour_id) = OPEN_TOUR() {
            selected.set(Some(tour_id));
            OPEN_TOUR.with_mut(|t| *t = None);
            deep_link_done.set(true);
        }
    });

    let search = use_signal(String::new);
    let mut page = use_signal(|| 0i64);
    // Cible de suppression (id, nom) — confirmation à la demande.
    let mut delete_target = use_signal(|| None::<(String, String)>);

    let can_write = current_role().is_some_and(|role| crate::state::org::role_can_write(&role));

    let list = use_resource(move || {
        let filters = api::tours::TourFilters {
            search: {
                let value = search().trim().to_string();
                (!value.is_empty()).then_some(value)
            },
            mode: None,
            limit: Some(PAGE_SIZE),
            offset: Some(page() * PAGE_SIZE),
        };
        async move {
            let _ = reload();
            api::tours::list(&filters).await
        }
    });

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

    // Colonnes de la table — les closures d'action capturent les signaux
    // (Copy) de la page.
    let columns = vec![
        Column::new(
            t!("studio-col-name").to_string(),
            move |tour: &TourSummary| {
                rsx! {
                    div { class: "flex items-center gap-2",
                        {tour.name.clone()}
                        span { class: "text-xs text-gray-400", {tour.mode.clone()} }
                    }
                }
            },
        )
        .with_td_class("font-medium text-gray-900"),
        Column::new(
            t!("studio-col-version").to_string(),
            |tour: &TourSummary| {
                rsx! { {format!("v{}", tour.latest_version_number)} }
            },
        )
        .with_td_class("text-sm text-gray-600").secondary(),
        Column::new(
            t!("studio-col-publication").to_string(),
            |tour: &TourSummary| {
                rsx! {
                    if let Some(version) = tour.published_version_number {
                        span { class: "inline-flex items-center px-2 py-0.5 rounded-full text-[11px] font-medium bg-green-50 text-green-700 border border-green-200",
                            {t!("studio-published-tag", version: version)}
                        }
                    } else {
                        span { class: "text-xs text-gray-400", {t!("studio-publish-none")} }
                    }
                }
            },
        ),
        Column::new(
            t!("studio-col-updated").to_string(),
            |tour: &TourSummary| {
                rsx! { {date_label(&tour.updated_at)} }
            },
        )
        .with_td_class("text-gray-500 text-sm").secondary(),
        Column::new(
            t!("common-actions").to_string(),
            move |tour: &TourSummary| {
                let id_open = tour.id.clone();
                let id_delete = tour.id.clone();
                let name_delete = tour.name.clone();
                rsx! {
                    div { class: "flex items-center gap-1.5",
                        button {
                            class: "px-3 py-1 text-sm bg-blue-100 text-blue-700 rounded-lg hover:bg-blue-200 transition-colors",
                            onclick: move |_| selected.set(Some(id_open.clone())),
                            {t!("studio-open")}
                        }
                        if can_write {
                            button {
                                class: "px-3 py-1 text-sm text-red-700 bg-red-50 border border-red-200 rounded-lg hover:bg-red-100 transition-colors",
                                onclick: move |_| delete_target.set(Some((id_delete.clone(), name_delete.clone()))),
                                icons::Trash2 { class: "h-3.5 w-3.5 inline mr-0.5" }
                                {t!("studio-delete")}
                            }
                        }
                    }
                }
            },
        ).actions(),
    ];

    rsx! {
        if let Some(tour_id) = selected() {
            // Éditeur plein écran, HORS ListLayout (principe 1 de la
            // coquille : plus d'« + Nouveau tour » en contexte d'édition).
            crate::components::tour_editor::TourEditor {
                key: "{tour_id}",
                tour_id,
                can_write,
                on_back: move |_| selected.set(None),
                on_changed: move |_| reload.with_mut(|r| *r += 1),
            }
        } else {
            ListLayout {
                title: t!("nav-studio").to_string(),
                subtitle: Some(t!("studio-subtitle").to_string()),
                on_refresh: move |_| reload.with_mut(|r| *r += 1),
                can_write,
                add_label: Some(t!("studio-new").to_string()),
                on_add: move |_| {
                    // Création immédiate, sans modale : nom daté,
                    // document minimal posé par le backend (un étage
                    // « RDC »), ouverture directe de l'éditeur.
                    let params = pnex_core::CreateTour {
                        name: t!("studio-default-name", date : crate ::util::now_label())
                            .to_string(),
                        description: None,
                        author: session::user().map(|user| user.username),
                        note: None,
                    };
                    spawn(async move {
                        match api::tours::create(params).await {
                            Ok(tour) => {
                                toasts::success("toast-tour-created");
                                selected.set(Some(tour.id));
                                reload.with_mut(|r| *r += 1);
                            }
                            Err(err) => toasts::error(err),
                        }
                    });
                },
                if org::current().is_none() {
                    p { class: "text-gray-500 text-center py-12", {t!("orgs-empty")} }
                } else {
                    // Recherche + refresh.
                    FilterBar {
                        SearchInput {
                            placeholder: t!("studio-search-placeholder").to_string(),
                            value: search,
                            on_submit: move |_| {
                                page.set(0);
                                reload.with_mut(|r| *r += 1);
                            },
                        }
                    }

                    ListStates {
                        state: list_state,
                        is_empty,
                        empty_message: t!("studio-empty").to_string(),
                        DataTable {
                            columns,
                            rows,
                            row_key: RowKey::new(|tour: &TourSummary| tour.id.clone()),
                        }
                        ListPager { count, page }
                    }
                    // Modal de création supprimée : « + Nouveau » crée
                    // directement (nom daté) et ouvre l'éditeur.

                    // Suppression confirmée d'une ligne.
                    if let Some((tour_id, tour_name)) = delete_target() {
                        ConfirmDialog {
                            title: t!("studio-confirm-delete-title"),
                            message: t!(
                                "common-quoted-message", name : tour_name.clone(), message :
                                t!("studio-confirm-delete-message")
                            ),
                            confirm_label: t!("studio-delete"),
                            on_confirm: move |_| {
                                delete_target.set(None);
                                let id = tour_id.clone();
                                spawn(async move {
                                    match api::tours::delete(&id).await {
                                        Ok(()) => {
                                            toasts::success("toast-tour-deleted");
                                            reload.with_mut(|r| *r += 1);
                                        }
                                        Err(err) => toasts::error(err),
                                    }
                                });
                            },
                            on_cancel: move |_| delete_target.set(None),
                        }
                    }
                }
            }
        }
    }
}
