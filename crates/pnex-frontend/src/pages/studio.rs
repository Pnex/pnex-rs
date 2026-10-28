//! Studio (3D tours) — list of the org's tours and editor
//! (`components/tour_editor/`, sub-view driven by the local signal
//! `selected` — `flows.rs` pattern).
//!
//! Org scoping comes from the client (`X-Org-Id`), writing is reserved
//! to owner/admin (the server enforces, the UI hides).

use dioxus::prelude::*;
use dioxus_i18n::t;
use pnex_core::TourSummary;

use crate::api;
use crate::components::badges::date_label;
use crate::components::confirm::ConfirmDialog;
use crate::components::crud::filters::{FilterBar, LabelFilterInput, SearchInput};
use crate::components::crud::layout::ListLayout;
use crate::components::crud::pager::{ListPager, PAGE_SIZE};
use crate::components::crud::states::ListStates;
use crate::components::crud::table::{Column, DataTable, RowKey};
use crate::components::icons;
use crate::components::labels_editor::{use_row_labels, LabelChips};
use crate::components::ui::BTN_DANGER;
use crate::state::tours::OPEN_TOUR;
use crate::state::{org, session, toasts};

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
    // D42: effective label filter ("name" or "name:value").
    let filter_label = use_signal(String::new);
    let mut page = use_signal(|| 0i64);
    // Deletion target (id, name) — confirmation on demand.
    let mut delete_target = use_signal(|| None::<(String, String)>);

    let can_write = crate::state::access::can("tours:write");

    let list = use_resource(move || {
        let filters = api::tours::TourFilters {
            search: {
                let value = search().trim().to_string();
                (!value.is_empty()).then_some(value)
            },
            mode: None,
            label: {
                let value = filter_label().trim().to_string();
                (!value.is_empty()).then_some(value)
            },
            limit: Some(PAGE_SIZE),
            offset: Some(page() * PAGE_SIZE),
        };
        async move {
            let _ = reload();
            api::tours::list(&filters).await
        }
    });

    // D42: own labels of the page rows, one batch per list (re)load.
    let row_labels = use_row_labels(pnex_core::resources::KIND_TOUR, move || {
        match &*list.value().read() {
            Some(Ok(paged)) => paged.results.iter().map(|t| t.id.clone()).collect(),
            _ => Vec::new(),
        }
    });

    // Synchronous read of the resource (CRUD base doctrine).
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

    // Table columns — the action closures capture the page's
    // (Copy) signals.
    let columns = vec![
        Column::new(
            t!("studio-col-name").to_string(),
            move |tour: &TourSummary| {
                let labels = row_labels.get(&tour.id).cloned().unwrap_or_default();
                rsx! {
                    div { class: "flex items-center gap-2",
                        {tour.name.clone()}
                        span { class: "text-xs text-gray-500", {tour.mode.clone()} }
                    }
                    LabelChips { labels }
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
                        span { class: "inline-flex items-center px-2 py-0.5 rounded-full text-xs font-medium bg-green-50 text-green-700 border border-green-200",
                            {t!("studio-published-tag", version: version)}
                        }
                    } else {
                        span { class: "text-xs text-gray-500", {t!("studio-publish-none")} }
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
                            class: crate::components::ui::BTN_SECONDARY_SM,
                            onclick: move |_| selected.set(Some(id_open.clone())),
                            {t!("studio-open")}
                        }
                        if can_write {
                            button {
                                class: BTN_DANGER,
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
            // Full-screen editor, OUTSIDE ListLayout (shell principle 1:
            // no more "+ New tour" in an editing context).
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
                    // Immediate creation, no modal: dated name,
                    // minimal document set by the backend (one
                    // "RDC" floor), editor opened directly.
                    let params = pnex_core::CreateTour {
                        name: default_name("studio-default-name"),
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
                            page,
                            placeholder: t!("studio-search-placeholder").to_string(),
                            value: search,
                            on_submit: move |_| {
                                page.set(0);
                                reload.with_mut(|r| *r += 1);
                            },
                        }
                        // D42: effective label filter (inheritance included).
                        LabelFilterInput {
                            placeholder: t!("resources-label-filter-placeholder").to_string(),
                            value: filter_label,
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
                    // Creation modal removed: "+ New" creates
                    // directly (dated name) and opens the editor.

                    // Confirmed deletion of a row.
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

/// Dated default name of a new item (stored data: plain text, no Fluent
/// bidi marks).
fn default_name(key: &str) -> String {
    crate::util::plain_text(t!(key, date: crate::util::now_label()))
}
