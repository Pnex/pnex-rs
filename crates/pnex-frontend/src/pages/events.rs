//! Events page (camera-video.md D84): JSON events written by the flow
//! `event-log` node into OpenObserve logs streams (`ev_…`) — stream, level,
//! full-text and period filters, D14 pagination, expandable JSON payload.
//! `available: false` → explanatory empty state (O2 not configured or no
//! event written yet).

use chrono::TimeZone as _;
use dioxus::prelude::*;
use dioxus_i18n::t;
use pnex_core::events::{EventLevel, EventRecord, DEFAULT_EVENT_STREAM};

use crate::api;
use crate::api::events::EventFilters;
use crate::app::Route;
use crate::components::crud::filters::{FilterBar, SearchInput};
use crate::components::crud::layout::ListLayout;
use crate::components::crud::pager::{ListPager, PAGE_SIZE};
use crate::components::crud::states::ListStates;
use crate::components::icons;
use crate::state::org;

/// Localized level name (also used by the `event-log` inspector).
pub(crate) fn level_label(level: EventLevel) -> String {
    match level {
        EventLevel::Debug => t!("events-level-debug").to_string(),
        EventLevel::Info => t!("events-level-info").to_string(),
        EventLevel::Warn => t!("events-level-warn").to_string(),
        EventLevel::Error => t!("events-level-error").to_string(),
    }
}

/// Level badge classes (full literals for the Tailwind scan).
fn level_classes(level: &str) -> &'static str {
    match level {
        "error" => "bg-red-100 text-red-700",
        "warn" => "bg-amber-100 text-amber-700",
        "debug" => "bg-gray-100 text-gray-600",
        _ => "bg-blue-100 text-blue-700",
    }
}

/// Unix µs → local `dd/mm HH:MM:SS`.
pub(crate) fn ts_label(ts_us: i64) -> String {
    chrono::Local
        .timestamp_micros(ts_us)
        .single()
        .map(|t| t.format("%d/%m %H:%M:%S").to_string())
        .unwrap_or_else(|| ts_us.to_string())
}

/// Period presets of the filter bar.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Period {
    Hour,
    Day,
    Week,
    Custom,
}

impl Period {
    fn wire(self) -> &'static str {
        match self {
            Self::Hour => "1h",
            Self::Day => "24h",
            Self::Week => "7d",
            Self::Custom => "custom",
        }
    }

    fn from_wire(s: &str) -> Self {
        match s {
            "1h" => Self::Hour,
            "7d" => Self::Week,
            "custom" => Self::Custom,
            _ => Self::Day,
        }
    }
}

/// `datetime-local` input value (`YYYY-MM-DDTHH:MM`) → RFC 3339 (local
/// offset).
pub(crate) fn local_input_to_rfc3339(raw: &str) -> Option<String> {
    let naive = chrono::NaiveDateTime::parse_from_str(raw.trim(), "%Y-%m-%dT%H:%M").ok()?;
    chrono::Local
        .from_local_datetime(&naive)
        .earliest()
        .map(|t| t.to_rfc3339())
}

/// RFC 3339 window of a period preset ending now; `Custom` uses the inputs.
fn window(period: Period, custom_from: &str, custom_to: &str) -> (Option<String>, Option<String>) {
    let now = chrono::Utc::now();
    let back = match period {
        Period::Hour => chrono::Duration::hours(1),
        Period::Day => chrono::Duration::hours(24),
        Period::Week => chrono::Duration::days(7),
        Period::Custom => {
            return (
                local_input_to_rfc3339(custom_from),
                local_input_to_rfc3339(custom_to),
            )
        }
    };
    (Some((now - back).to_rfc3339()), Some(now.to_rfc3339()))
}

#[component]
pub fn Events() -> Element {
    let mut stream = use_signal(|| DEFAULT_EVENT_STREAM.to_string());
    let mut level = use_signal(String::new);
    let search = use_signal(String::new);
    let mut period = use_signal(|| Period::Day);
    let mut custom_from = use_signal(String::new);
    let mut custom_to = use_signal(String::new);
    let mut page = use_signal(|| 0i64);
    let mut reload = use_signal(|| 0u32);

    let streams = use_resource(move || async move {
        let _ = org::ORG.read();
        api::events::streams().await.unwrap_or_default()
    });
    let events = use_resource(move || {
        let (from, to) = window(period(), &custom_from(), &custom_to());
        let filters = EventFilters {
            stream: Some(stream()),
            level: Some(level()),
            q: Some(search()),
            from,
            to,
            limit: Some(PAGE_SIZE),
            offset: Some(page() * PAGE_SIZE),
        };
        async move {
            let _ = reload();
            let _ = org::ORG.read();
            api::events::list(&filters).await
        }
    });

    let mut stream_options = streams.value().read().clone().unwrap_or_default();
    if !stream_options.iter().any(|s| s == DEFAULT_EVENT_STREAM) {
        stream_options.insert(0, DEFAULT_EVENT_STREAM.to_string());
    }
    let (list_state, available, is_empty, count, rows) = match &*events.value().read() {
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

    rsx! {
        ListLayout {
            title: t!("events-title").to_string(),
            subtitle: Some(t!("events-subtitle").to_string()),
            on_refresh: move |_| reload.with_mut(|r| *r += 1),
            can_write: false,
            FilterBar {
                select {
                    aria_label: t!("events-stream"),
                    class: "px-3 py-2 border border-gray-300 rounded-lg text-sm bg-white",
                    onchange: move |e| {
                        stream.set(e.value());
                        page.set(0);
                    },
                    for s in stream_options {
                        option {
                            key: "{s}",
                            value: "{s}",
                            selected: s == stream(),
                            "{s}"
                        }
                    }
                }
                select {
                    aria_label: t!("events-col-level"),
                    class: "px-3 py-2 border border-gray-300 rounded-lg text-sm bg-white",
                    onchange: move |e| {
                        level.set(e.value());
                        page.set(0);
                    },
                    option { value: "", selected: level().is_empty(), {t!("events-level-all")} }
                    for l in EventLevel::ALL {
                        option {
                            key: "{l.wire()}",
                            value: l.wire(),
                            selected: level() == l.wire(),
                            {level_label(l)}
                        }
                    }
                }
                select {
                    aria_label: t!("events-period"),
                    class: "px-3 py-2 border border-gray-300 rounded-lg text-sm bg-white",
                    onchange: move |e| {
                        period.set(Period::from_wire(&e.value()));
                        page.set(0);
                    },
                    for (p, label) in [
                        (Period::Hour, t!("events-period-1h")),
                        (Period::Day, t!("events-period-24h")),
                        (Period::Week, t!("events-period-7d")),
                        (Period::Custom, t!("events-period-custom")),
                    ]
                    {
                        option {
                            key: "{p.wire()}",
                            value: p.wire(),
                            selected: period() == p,
                            {label}
                        }
                    }
                }
                if period() == Period::Custom {
                    input {
                        class: "px-3 py-2 border border-gray-300 rounded-lg text-sm",
                        r#type: "datetime-local",
                        title: t!("events-from"),
                        value: "{custom_from}",
                        onchange: move |e| {
                            custom_from.set(e.value());
                            page.set(0);
                        },
                    }
                    input {
                        class: "px-3 py-2 border border-gray-300 rounded-lg text-sm",
                        r#type: "datetime-local",
                        title: t!("events-to"),
                        value: "{custom_to}",
                        onchange: move |e| {
                            custom_to.set(e.value());
                            page.set(0);
                        },
                    }
                }
                SearchInput {
                    placeholder: t!("events-search-placeholder").to_string(),
                    value: search,
                    on_submit: move |_| page.set(0),
                }
            }
            if !available {
                div { class: "text-center py-12 bg-white rounded-lg shadow border border-gray-200",
                    icons::History { class: "h-8 w-8 text-gray-400 mx-auto" }
                    p { class: "text-gray-900 font-medium mt-3", {t!("events-unavailable-title")} }
                    p { class: "text-gray-600 mt-2 max-w-xl mx-auto text-sm",
                        {t!("events-unavailable-message")}
                    }
                }
            } else {
                ListStates {
                    state: list_state,
                    is_empty,
                    empty_message: t!("events-empty-title").to_string(),
                    empty_icon: rsx! {
                        icons::History { class: "h-8 w-8 text-gray-400" }
                    },
                    empty_detail: rsx! {
                        p { class: "text-gray-600 mt-2 max-w-xl mx-auto", {t!("events-empty-message")} }
                    },
                    div { class: "space-y-4",
                        div { class: "overflow-x-auto bg-white rounded-lg shadow border border-gray-200",
                            table { class: "min-w-full divide-y divide-gray-200 text-sm",
                                thead { class: "bg-gray-50",
                                    tr {
                                        th { class: "px-4 py-2 text-left font-medium text-gray-600",
                                            {t!("events-col-time")}
                                        }
                                        th { class: "px-4 py-2 text-left font-medium text-gray-600",
                                            {t!("events-col-level")}
                                        }
                                        th { class: "px-4 py-2 text-left font-medium text-gray-600",
                                            {t!("events-col-message")}
                                        }
                                        th { class: "px-4 py-2 text-left font-medium text-gray-600",
                                            {t!("events-col-topic")}
                                        }
                                        th { class: "px-4 py-2 text-left font-medium text-gray-600",
                                            {t!("events-col-flow")}
                                        }
                                        th { class: "px-4 py-2 text-left font-medium text-gray-600",
                                            {t!("events-col-node")}
                                        }
                                    }
                                }
                                tbody { class: "divide-y divide-gray-100",
                                    for (i, ev) in rows.into_iter().enumerate() {
                                        EventRow { key: "{ev.ts_us}-{i}", ev }
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
}

#[component]
fn EventRow(ev: EventRecord) -> Element {
    let mut open = use_signal(|| false);
    let navigator = use_navigator();
    let payload = serde_json::to_string_pretty(&ev.payload).unwrap_or_default();
    let has_payload = !ev.payload.is_null();
    let flow_id = ev.flow_id;

    rsx! {
        tr {
            class: "hover:bg-gray-50 cursor-pointer",
            onclick: move |_| {
                let next = !open();
                open.set(next);
            },
            td { class: "px-4 py-2 text-gray-600 whitespace-nowrap",
                if open() {
                    icons::ChevronDown { class: "h-3 w-3 inline mr-1 text-gray-400" }
                } else {
                    icons::ChevronRight { class: "h-3 w-3 inline mr-1 text-gray-400" }
                }
                {ts_label(ev.ts_us)}
            }
            td { class: "px-4 py-2",
                span { class: "px-2 py-0.5 rounded-full text-xs {level_classes(&ev.level)}",
                    {
                        EventLevel::from_wire(&ev.level)
                            .map(level_label)
                            .unwrap_or_else(|| ev.level.clone())
                    }
                }
            }
            td { class: "px-4 py-2 text-gray-900", "{ev.message}" }
            td { class: "px-4 py-2 text-gray-600", {ev.topic.clone().unwrap_or_default()} }
            td { class: "px-4 py-2",
                if let Some(id) = flow_id {
                    button {
                        class: "text-blue-600 hover:underline",
                        r#type: "button",
                        onclick: move |e| {
                            e.stop_propagation();
                            crate::state::flows::OPEN_FLOW.with_mut(|v| *v = Some(id));
                            navigator.push(Route::Flows {});
                        },
                        "#{id}"
                    }
                }
            }
            td { class: "px-4 py-2 text-gray-500 font-mono text-xs", "{ev.node_id}" }
        }
        if open() {
            tr {
                td { class: "px-4 pb-3 bg-gray-50", colspan: "6",
                    if has_payload {
                        pre { class: "text-xs font-mono bg-white border border-gray-200 rounded-lg p-3 overflow-x-auto max-h-80",
                            "{payload}"
                        }
                    } else {
                        p { class: "text-xs text-gray-500", {t!("events-no-payload")} }
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn period_wire_roundtrip() {
        for p in [Period::Hour, Period::Day, Period::Week, Period::Custom] {
            assert_eq!(Period::from_wire(p.wire()), p);
        }
    }

    #[test]
    fn custom_window_parses_local_inputs() {
        let (from, to) = window(Period::Custom, "2026-09-29T08:00", "");
        assert!(from.unwrap().starts_with("2026-09-29T08:00:00"));
        assert!(to.is_none());
        let (from, to) = window(Period::Hour, "", "");
        assert!(from.is_some() && to.is_some());
    }
}
