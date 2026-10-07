//! "Events" tab of `/notifications` (D86): every delivery attempt of the
//! org, read from the OpenObserve journal — channel, status, source,
//! period and full-text filters, D14 pagination, expandable detail
//! (upstream return, error, node, reached sessions).

use dioxus::prelude::*;
use dioxus_i18n::t;
use pnex_core::{NotifyChannel, NotifyDelivery, NotifyTemplate};

use crate::api;
use crate::api::notify::DeliveryFilters;
use crate::app::Route;
use crate::components::crud::filters::{FilterBar, SearchInput};
use crate::components::crud::pager::{ListPager, PAGE_SIZE};
use crate::components::crud::states::ListStates;
use crate::components::icons;
use crate::pages::events::ts_label;
use crate::state::org;

const STATUSES: [&str; 3] = ["sent", "failed", "blocked"];
const SOURCES: [&str; 3] = ["flow", "test", "ota"];

/// Localized delivery status.
pub(crate) fn status_label(status: &str) -> String {
    match status {
        "sent" => t!("notify-status-sent").to_string(),
        "failed" => t!("notify-status-failed").to_string(),
        "blocked" => t!("notify-status-blocked").to_string(),
        other => other.to_string(),
    }
}

/// Status badge classes (full literals for the Tailwind scan).
pub(crate) fn status_classes(status: &str) -> &'static str {
    match status {
        "sent" => "bg-green-50 text-green-700",
        "blocked" => "bg-amber-50 text-amber-700",
        _ => "bg-red-50 text-red-700",
    }
}

fn source_label(source: &str) -> String {
    match source {
        "flow" => t!("notify-source-flow").to_string(),
        "test" => t!("notify-source-test").to_string(),
        "ota" => t!("notify-source-ota").to_string(),
        other => other.to_string(),
    }
}

/// Period presets, in hours back from now.
const PERIODS: [(i64, &str); 4] = [
    (1, "events-period-1h"),
    (24, "events-period-24h"),
    (24 * 7, "events-period-7d"),
    (24 * 30, "notify-period-30d"),
];

#[component]
pub fn NotifyDeliveriesTab(
    channels: Vec<NotifyChannel>,
    templates: Vec<NotifyTemplate>,
    mut channel_filter: Signal<String>,
    reload: Signal<u32>,
) -> Element {
    let mut status = use_signal(String::new);
    let mut source = use_signal(String::new);
    let search = use_signal(String::new);
    let mut hours = use_signal(|| 24i64);
    let mut page = use_signal(|| 0i64);

    let deliveries = use_resource(move || {
        let now = chrono::Utc::now();
        let filters = DeliveryFilters {
            channel_id: Some(channel_filter()),
            status: Some(status()),
            source: Some(source()),
            q: Some(search()),
            from: Some((now - chrono::Duration::hours(hours())).to_rfc3339()),
            to: Some(now.to_rfc3339()),
            limit: Some(PAGE_SIZE),
            offset: Some(page() * PAGE_SIZE),
        };
        async move {
            let _ = reload();
            let _ = org::ORG.read();
            api::notify::list_deliveries(&filters).await
        }
    });

    let (list_state, available, is_empty, count, rows) = match &*deliveries.value().read() {
        None => (None, true, false, 0, Vec::new()),
        Some(Ok(p)) => (
            Some(Ok(())),
            p.available,
            p.results.is_empty(),
            p.count,
            p.results.clone(),
        ),
        Some(Err(err)) => (Some(Err(err.clone())), true, false, 0, Vec::new()),
    };
    let names: std::collections::HashMap<String, String> = channels
        .iter()
        .map(|c| (c.id.to_string(), c.name.clone()))
        .collect();
    let template_names: std::collections::HashMap<String, String> = templates
        .iter()
        .map(|t| (t.id.to_string(), t.name.clone()))
        .collect();

    rsx! {
        FilterBar {
            select {
                class: "px-3 py-2 border border-gray-300 rounded-lg text-sm bg-white",
                onchange: move |e| {
                    channel_filter.set(e.value());
                    page.set(0);
                },
                option { value: "", selected: channel_filter().is_empty(),
                    {t!("notify-filter-channel-all")}
                }
                for c in channels.clone() {
                    option {
                        key: "{c.id}",
                        value: "{c.id}",
                        selected: channel_filter() == c.id.to_string(),
                        "{c.name}"
                    }
                }
            }
            select {
                class: "px-3 py-2 border border-gray-300 rounded-lg text-sm bg-white",
                onchange: move |e| {
                    status.set(e.value());
                    page.set(0);
                },
                option { value: "", selected: status().is_empty(), {t!("notify-filter-status-all")} }
                for s in STATUSES {
                    option { key: "{s}", value: s, selected: status() == s, {status_label(s)} }
                }
            }
            select {
                class: "px-3 py-2 border border-gray-300 rounded-lg text-sm bg-white",
                onchange: move |e| {
                    source.set(e.value());
                    page.set(0);
                },
                option { value: "", selected: source().is_empty(), {t!("notify-filter-source-all")} }
                for s in SOURCES {
                    option { key: "{s}", value: s, selected: source() == s, {source_label(s)} }
                }
            }
            select {
                class: "px-3 py-2 border border-gray-300 rounded-lg text-sm bg-white",
                onchange: move |e| {
                    hours.set(e.value().parse().unwrap_or(24));
                    page.set(0);
                },
                for (h, key) in PERIODS {
                    option { key: "{h}", value: "{h}", selected: hours() == h, {t!(key)} }
                }
            }
            SearchInput {
                placeholder: t!("notify-deliveries-search").to_string(),
                value: search,
                on_submit: move |_| page.set(0),
            }
        }
        if !available {
            div { class: "text-center py-12 bg-white rounded-lg shadow border border-gray-200",
                icons::History { class: "h-8 w-8 text-gray-400 mx-auto" }
                p { class: "text-gray-900 font-medium mt-3",
                    {t!("notify-deliveries-unavailable-title")}
                }
                p { class: "text-gray-600 mt-2 max-w-xl mx-auto text-sm",
                    {t!("notify-deliveries-unavailable-message")}
                }
            }
        } else {
            ListStates {
                state: list_state,
                is_empty,
                empty_message: t!("notify-deliveries-empty").to_string(),
                empty_icon: rsx! {
                    icons::History { class: "h-8 w-8 text-gray-400" }
                },
                div { class: "space-y-4",
                    div { class: "overflow-x-auto bg-white rounded-lg shadow border border-gray-200",
                        table { class: "min-w-full divide-y divide-gray-200 text-sm",
                            thead { class: "bg-gray-50",
                                tr {
                                    th { class: "th", {t!("notify-col-date")} }
                                    th { class: "th", {t!("notify-col-channel")} }
                                    th { class: "th", {t!("notify-col-status")} }
                                    th { class: "th hidden md:table-cell", {t!("notify-col-source")} }
                                    th { class: "th min-w-[10rem]", {t!("notify-col-subject")} }
                                    th { class: "th hidden md:table-cell", {t!("notify-col-flow")} }
                                }
                            }
                            tbody { class: "divide-y divide-gray-100",
                                for (i, d) in rows.into_iter().enumerate() {
                                    DeliveryRow {
                                        key: "{d.ts_us}-{i}",
                                        channel_name: names.get(&d.channel_id.to_string()).cloned(),
                                        template_name: name_of(&template_names, d.template_id),
                                        d,
                                    }
                                }
                            }
                        }
                    }
                    ListPager { count, page }
                }
            }
        }
    }
}

#[component]
fn DeliveryRow(
    d: NotifyDelivery,
    channel_name: Option<String>,
    template_name: Option<String>,
) -> Element {
    let mut open = use_signal(|| false);
    let navigator = use_navigator();
    let flow_id = d.flow_id;
    let channel = channel_name.unwrap_or_else(|| t!("notify-deleted-channel").to_string());

    rsx! {
        tr {
            class: "hover:bg-gray-50 cursor-pointer",
            onclick: move |_| {
                let next = !open();
                open.set(next);
            },
            td { class: "td text-gray-600 whitespace-nowrap",
                if open() {
                    icons::ChevronDown { class: "h-3 w-3 inline mr-1 text-gray-400" }
                } else {
                    icons::ChevronRight { class: "h-3 w-3 inline mr-1 text-gray-400" }
                }
                {ts_label(d.ts_us)}
            }
            td { class: "td text-gray-900",
                "{channel}"
                span { class: "ml-2 text-xs text-gray-500", "{d.channel_kind}" }
            }
            td { class: "td whitespace-nowrap",
                span { class: "px-2 py-0.5 rounded-full text-xs {status_classes(&d.status)}",
                    {status_label(&d.status)}
                    if let Some(code) = d.http_status {
                        " · HTTP {code}"
                    }
                }
            }
            td { class: "td hidden text-gray-600 md:table-cell", {source_label(&d.source)} }
            td { class: "td min-w-[10rem] text-gray-700 break-words",
                {d.subject.clone().unwrap_or_default()}
            }
            td { class: "td hidden md:table-cell",
                if let Some(id) = flow_id {
                    button {
                        class: "text-blue-600 hover:underline",
                        r#type: "button",
                        onclick: move |e| {
                            e.stop_propagation();
                            navigator.push(Route::Flows { id: id.to_string() });
                        },
                        "#{id}"
                    }
                }
            }
        }
        if open() {
            tr {
                td {
                    class: "px-3 pb-3 bg-gray-50 text-xs text-gray-700 md:px-4",
                    colspan: "6",
                    dl { class: "grid grid-cols-[max-content_1fr] gap-x-4 gap-y-1 pt-2",
                        dt { class: "text-gray-500 md:hidden", {t!("notify-col-source")} }
                        dd { class: "md:hidden", {source_label(&d.source)} }
                        if let Some(id) = flow_id {
                            dt { class: "text-gray-500 md:hidden", {t!("notify-col-flow")} }
                            dd { class: "md:hidden", "#{id}" }
                        }
                        if let Some(err) = d.error.clone() {
                            dt { class: "text-gray-500", {t!("notify-detail-error")} }
                            dd { class: "font-mono text-red-700 break-all", "{err}" }
                        }
                        if let Some(node) = d.node_id.clone() {
                            dt { class: "text-gray-500", {t!("notify-detail-node")} }
                            dd { class: "font-mono", "{node}" }
                        }
                        if let Some(n) = d.delivered {
                            dt { class: "text-gray-500", {t!("notify-detail-sessions")} }
                            dd { "{n}" }
                        }
                        if let Some(tpl) = d.template_id {
                            dt { class: "text-gray-500", {t!("notify-detail-template")} }
                            dd { {template_name.clone().unwrap_or_else(|| tpl.to_string())} }
                        }
                        dt { class: "text-gray-500", {t!("notify-detail-channel-id")} }
                        dd { class: "font-mono", "{d.channel_id}" }
                    }
                }
            }
        }
    }
}

/// Display name for an optional id, looked up in an id → name map.
fn name_of(
    names: &std::collections::HashMap<String, String>,
    id: Option<impl ToString>,
) -> Option<String> {
    id.and_then(|id| names.get(&id.to_string()).cloned())
}
