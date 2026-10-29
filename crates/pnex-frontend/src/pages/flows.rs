//! ETL flows (D18) — list of the org's flows (filters + D14 pagination) and
//! drag & drop editor (`components/flow_editor/`, sub-view driven by the
//! local `selected` signal — `devices.rs` pattern). The page composes the
//! CRUD base (`components/crud/` — module README for the doctrine):
//! ListLayout / FilterBar / ListStates / DataTable / ListPager.
//!
//! Org scoping comes from the client (`X-Org-Id`), writing is reserved to
//! owner/admin (the server enforces, the UI hides). Creation starts the flow
//! with a default inject node: the API refuses an empty graph
//! (`empty_graph`).

use dioxus::prelude::*;
use dioxus_i18n::t;
use pnex_core::{FlowGraph, FlowNode, FlowNodeKind, FlowSummary, InjectConfig, Position};

use crate::api;
use crate::components::badges::date_label;
use crate::components::confirm::ConfirmDialog;
use crate::components::crud::filters::{FilterBar, LabelFilterInput, SearchInput};
use crate::components::crud::layout::{ListLayout, DANGER_BTN};
use crate::components::crud::pager::{ListPager, PAGE_SIZE};
use crate::components::crud::states::ListStates;
use crate::components::crud::table::{Column, DataTable, RowKey};
use crate::components::gerbe::Gerbe;
use crate::components::icons;
use crate::components::labels_editor::{use_row_labels, LabelChips};
use crate::components::loading_overlay::LoadingOverlay;
use crate::components::ui::INPUT;
use crate::state::{org, session, toasts};

#[component]
pub fn Flows(id: String) -> Element {
    let mut reload = use_signal(|| 0u32);
    // The open flow mirrors the `?id=` of the URL (O31): deep links (search,
    // assistant, events, "Create and open the flow") and a reload land on
    // it, the nav link (no id) goes back to the list.
    let mut selected = use_signal(|| id.parse::<i64>().ok());
    use_effect(use_reactive((&id,), move |(id,)| {
        let want = id.parse::<i64>().ok();
        if *selected.peek() != want {
            selected.set(want);
        }
    }));
    use_effect(move || {
        let want = selected().map(|f| f.to_string()).unwrap_or_default();
        if let crate::app::Route::Flows { id } = router().current::<crate::app::Route>() {
            if id != want {
                navigator().replace(crate::app::Route::Flows { id: want });
            }
        }
    });
    let mut filter_status = use_signal(|| "all".to_string());
    let search = use_signal(String::new);
    // D42: effective label filter ("name" or "name:value").
    let filter_label = use_signal(String::new);
    let mut page = use_signal(|| 0i64);
    // Deletion target (id, name) — confirmation on demand.
    let mut delete_target = use_signal(|| None::<(i64, String)>);
    // Deletion in progress (id): the delete waits for the runtime ack —
    // the row shows a spinner so it does not look like a lost click.
    let mut deleting = use_signal(|| None::<i64>);

    let can_write = crate::state::access::can("flows:write");

    let list = use_resource(move || {
        let filters = api::flows::FlowFilters {
            search: {
                let value = search().trim().to_string();
                if value.is_empty() {
                    None
                } else {
                    Some(value)
                }
            },
            status: match filter_status().as_str() {
                "all" => None,
                other => Some(other.to_string()),
            },
            label: {
                let value = filter_label().trim().to_string();
                (!value.is_empty()).then_some(value)
            },
            limit: Some(PAGE_SIZE),
            offset: Some(page() * PAGE_SIZE),
        };
        async move {
            let _ = reload();
            api::flows::list(&filters).await
        }
    });

    // D42: own labels of the page rows, one batch per list (re)load.
    let row_labels = use_row_labels(pnex_core::resources::KIND_FLOW, move || {
        match &*list.value().read() {
            Some(Ok(paged)) => paged.results.iter().map(|f| f.id.to_string()).collect(),
            _ => Vec::new(),
        }
    });

    // Synchronous read of the resource (CRUD base doctrine: the state
    // discriminant and the rows come out of the match before the rsx).
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

    // Table columns — the action closures capture the page's (Copy)
    // signals; historical td classes via with_td_class.
    let columns = vec![
        Column::new(t!("flows-col-name").to_string(), move |flow: &FlowSummary| {
            let labels = row_labels.get(&flow.id.to_string()).cloned().unwrap_or_default();
            rsx! {
                {flow.name.clone()}
                LabelChips { labels }
            }
        })
        .with_td_class("font-medium text-gray-900"),
        Column::new(t!("flows-col-status").to_string(), |flow: &FlowSummary| {
            let (badge, label) = status_badge(flow.status.as_str());
            rsx! {
                span { class: "inline-flex items-center px-2.5 py-0.5 rounded-full text-xs font-medium {badge} w-fit",
                    {label}
                }
            }
        }),
        Column::new(t!("flows-col-versions").to_string(), |flow: &FlowSummary| {
            rsx! {
                div { class: "flex items-center gap-2",
                    span { {format!("v{}", flow.latest_version_number)} }
                    if let Some(deployed) = flow.deployed_version_number {
                        span { class: "inline-flex items-center px-2 py-0.5 rounded-full text-xs font-medium bg-green-50 text-green-700 border border-green-200",
                            {format!("v{deployed} {}", t!("flows-version-deployed-tag"))}
                        }
                    }
                }
            }
        })
        .with_td_class("text-sm text-gray-600").secondary(),
        Column::new(t!("flows-col-updated").to_string(), |flow: &FlowSummary| {
            rsx! { {date_label(&flow.updated_at)} }
        })
        .with_td_class("text-gray-500 text-sm").secondary(),
        Column::new(
            t!("common-actions").to_string(),
            move |flow: &FlowSummary| {
                let pk = flow.id;
                // Cloned BEFORE the onclick closure: the `flow` reference
                // does not leave the cell body (E0521).
                let name_delete = flow.name.clone();
                let is_deleting = deleting() == Some(pk);
                rsx! {
                    div { class: "flex items-center gap-1.5",
                        button {
                            class: crate::components::ui::BTN_SECONDARY_SM,
                            disabled: is_deleting,
                            onclick: move |_| selected.set(Some(pk)),
                            {t!("flows-open")}
                        }
                        if can_write {
                            button {
                                class: format!("{DANGER_BTN} disabled:opacity-40 disabled:cursor-not-allowed"),
                                disabled: is_deleting,
                                onclick: move |_| delete_target.set(Some((pk, name_delete.clone()))),
                                if is_deleting {
                                    Gerbe { size: 14, class: "mr-0.5" }
                                } else {
                                    icons::Trash2 { class: "h-3.5 w-3.5 inline mr-0.5" }
                                }
                                {if is_deleting { t!("flows-deleting") } else { t!("flows-delete") }}
                            }
                        }
                    }
                }
            },
        ).actions(),
    ];

    rsx! {
        if let Some(flow_id) = selected() {
            // Full-screen editor, OUTSIDE ListLayout — no more
            // "+ New flow" button in editing context (principle 1 of the
            // editor shell: adding lives in the canvas).
            crate::components::flow_editor::FlowEditor {
                key: "{flow_id}",
                flow_id,
                can_write,
                on_back: move |_| {
                    selected.set(None);
                    // Back to the list: systematic refetch —
                    // the server truth (flow created, renamed,
                    // deploys done in the editor) shows up without
                    // going through the refresh button.
                    reload.with_mut(|r| *r += 1);
                },
                on_changed: move |_| reload.with_mut(|r| *r += 1),
            }
        } else {
            ListLayout {
                title: t!("nav-flows").to_string(),
                subtitle: Some(t!("flows-subtitle").to_string()),
                on_refresh: move |_| reload.with_mut(|r| *r += 1),
                can_write,
                add_label: Some(t!("flows-new").to_string()),
                on_add: move |_| {
                    // Immediate creation, no modal: dated name + starting
                    // graph, then the editor opens directly
                    // (fluidity — user feedback 2026-09-18). Renaming
                    // stays possible afterwards.
                    let params = pnex_core::CreateFlow {
                        name: default_name("flows-default-name"),
                        device_id: None,
                        graph: starter_graph(),
                        author: session::user().map(|user| user.username),
                        note: None,
                    };
                    spawn(async move {
                        match api::flows::create(params).await {
                            Ok(flow) => {
                                toasts::success("toast-flow-created");
                                selected.set(Some(flow.id));
                                reload.with_mut(|r| *r += 1);
                            }
                            Err(err) => toasts::error(err),
                        }
                    });
                },
                if org::current().is_none() {
                    p { class: "text-gray-500 text-center py-12", {t!("orgs-empty")} }
                } else {
                    FilterBar {
                        select {
                            aria_label: t!("flows-col-status"),
                            class: INPUT,
                            onchange: move |event| {
                                filter_status.set(event.value());
                                page.set(0);
                                reload.with_mut(|r| *r += 1);
                            },
                            option {
                                value: "all",
                                selected: filter_status() == "all",
                                {t!("flows-filter-status-all")}
                            }
                            option {
                                value: "draft",
                                selected: filter_status() == "draft",
                                {t!("flows-status-draft")}
                            }
                            option {
                                value: "deployed",
                                selected: filter_status() == "deployed",
                                {t!("flows-status-deployed")}
                            }
                            option {
                                value: "stopped",
                                selected: filter_status() == "stopped",
                                {t!("flows-status-stopped")}
                            }
                            option {
                                value: "error",
                                selected: filter_status() == "error",
                                {t!("flows-status-error")}
                            }
                        }
                        SearchInput {
                            page,
                            placeholder: t!("flows-search-placeholder").to_string(),
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
                        empty_message: t!("flows-empty").to_string(),
                        div { class: "relative",
                            DataTable {
                                columns,
                                rows,
                                row_key: RowKey::new(|flow: &FlowSummary| flow.id.to_string()),
                            }
                            // Server operation in progress (delete waits
                            // for the runtime ack) — global feedback.
                            if deleting().is_some() {
                                LoadingOverlay { message: t!("flows-deleting").to_string() }
                            }
                        }
                        ListPager { count, page }
                    }
                    // Creation modal removed: "+ New" creates
                    // directly (dated name) and opens the editor.

                    // Confirmed deletion of a row.
                    if let Some((flow_id, flow_name)) = delete_target() {
                        ConfirmDialog {
                            title: t!("flows-confirm-delete-title"),
                            message: t!(
                                "common-quoted-message", name : flow_name.clone(), message :
                                t!("flows-confirm-delete-message")
                            ),
                            confirm_label: t!("flows-delete"),
                            on_confirm: move |_| {
                                delete_target.set(None);
                                deleting.set(Some(flow_id));
                                spawn(async move {
                                    // The list is refreshed in both
                                    // cases: it reflects the server truth
                                    // (a failure = the flow still exists).
                                    let outcome = api::flows::delete(flow_id).await;
                                    deleting.set(None);
                                    match outcome {
                                        Ok(()) => toasts::success("toast-flow-deleted"),
                                        Err(err) => toasts::error(err),
                                    }
                                    reload.with_mut(|r| *r += 1);
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

/// Status badge — full literal classes (Tailwind scan).
pub(crate) fn status_badge(status: &str) -> (&'static str, String) {
    let label = match status {
        "deployed" => t!("flows-status-deployed"),
        "stopped" => t!("flows-status-stopped"),
        "error" => t!("flows-status-error"),
        _ => t!("flows-status-draft"),
    };
    let badge = match status {
        "deployed" => "bg-green-100 text-green-800",
        "stopped" => "bg-amber-100 text-amber-800",
        "error" => "bg-red-100 text-red-800",
        _ => "bg-gray-100 text-gray-800",
    };
    (badge, label)
}

/// Starting graph of a new flow: an inject node in **continuous
/// repetition** (the API refuses an empty graph — `empty_graph` violation).
pub(crate) fn starter_graph() -> FlowGraph {
    FlowGraph {
        nodes: vec![FlowNode {
            id: "n1".into(),
            name: None,
            position: Some(Position { x: 100.0, y: 100.0 }),
            outputs: vec![],
            inputs: vec![],
            kind: FlowNodeKind::Inject {
                config: InjectConfig {
                    repeat_secs: Some(30.0),
                    ..Default::default()
                },
            },
        }],
    }
}

/// Dated default name of a new item (stored data: plain text, no Fluent
/// bidi marks).
fn default_name(key: &str) -> String {
    crate::util::plain_text(t!(key, date: crate::util::now_label()))
}
