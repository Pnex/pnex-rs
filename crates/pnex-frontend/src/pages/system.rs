//! System page (D72) — telemetry data of the current org: effective O2
//! retention (read-only; set in Platform status) and cleanup (per stream,
//! bulk selection, time range, full purge — owner/admin; the server
//! enforces, the UI hides). Sizes are on-disk (compressed) sizes.

use dioxus::prelude::*;
use dioxus_i18n::t;
use pnex_core::{O2StreamInfo, RetentionInfo};

use crate::api;
use crate::components::confirm::ConfirmDialog;
use crate::components::crud::layout::{ListLayout, DANGER_BTN};
use crate::components::crud::pager::{ListPager, PAGE_SIZE};
use crate::components::crud::states::ListStates;
use crate::components::crud::table::{Column, DataTable, RowKey};
use crate::components::icons;
use crate::state::{org, session, toasts};

/// Human-readable size (B/KB/MB/GB/TB).
pub fn format_bytes(bytes: f64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut value = bytes.max(0.0);
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{value:.0} B")
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

/// `2026-09-28T10:17:05+00:00` → `2026-09-28 10:17` (UTC, compact).
fn short_utc(ts: &Option<String>) -> String {
    ts.as_deref()
        .map(|s| s.get(..16).unwrap_or(s).replace('T', " "))
        .unwrap_or_else(|| "—".into())
}

fn current_role() -> Option<String> {
    let user = session::user()?;
    let org_id = org::current()?;
    user.orgs
        .iter()
        .find(|m| m.id == org_id)
        .map(|m| m.role.clone())
}

fn current_org_name() -> String {
    let org_id = org::current();
    session::user()
        .and_then(|u| u.orgs.into_iter().find(|m| Some(m.id) == org_id))
        .map(|m| m.name)
        .unwrap_or_default()
}

/// Pending destructive action awaiting confirmation.
#[derive(Clone, PartialEq)]
enum Pending {
    DeleteStream(String),
    /// Bulk deletion of the selected streams.
    DeleteStreams(Vec<String>),
    DeleteRange(String),
    Purge,
}

#[component]
pub fn System() -> Element {
    let mut reload = use_signal(|| 0u32);
    let mut pending = use_signal(|| None::<Pending>);
    let can_write = current_role().is_some_and(|r| crate::state::org::role_can_administer(&r));

    let retention = use_resource(move || async move {
        let _ = reload();
        let _ = org::current();
        api::system::retention().await
    });
    let streams = use_resource(move || async move {
        let _ = reload();
        let _ = org::current();
        api::system::o2_streams().await
    });

    let mut bump = move || reload.with_mut(|r| *r += 1);

    rsx! {
        ListLayout {
            title: t!("system-title").to_string(),
            subtitle: Some(t!("system-subtitle").to_string()),
            on_refresh: move |_| bump(),
            can_write,
            div { class: "space-y-6",
                match &*retention.value().read() {
                    None => rsx! {
                        LoadingCard {}
                    },
                    Some(Err(err)) => rsx! {
                        ErrorCard { message: api::error_i18n::localize(err) }
                    },
                    Some(Ok(info)) => rsx! {
                        RetentionCard { info: info.clone() }
                    },
                }
                crate::components::ai_retention::AiRetentionCard { platform: false }
                DataCard {
                    streams: streams.value().read().clone(),
                    can_write,
                    on_action: move |p| pending.set(Some(p)),
                }
            }
        }
        match pending() {
            Some(Pending::DeleteStream(name)) => rsx! {
                DeleteStreamDialog {
                    name,
                    on_done: move |_| {
                        pending.set(None);
                        bump();
                    },
                    on_cancel: move |_| pending.set(None),
                }
            },
            Some(Pending::DeleteStreams(names)) => rsx! {
                DeleteStreamsDialog {
                    names,
                    on_done: move |_| {
                        pending.set(None);
                        bump();
                    },
                    on_cancel: move |_| pending.set(None),
                }
            },
            Some(Pending::DeleteRange(name)) => rsx! {
                DeleteRangeDialog {
                    name,
                    on_done: move |_| {
                        pending.set(None);
                        bump();
                    },
                    on_cancel: move |_| pending.set(None),
                }
            },
            Some(Pending::Purge) => rsx! {
                PurgeDialog {
                    on_done: move |_| {
                        pending.set(None);
                        bump();
                    },
                    on_cancel: move |_| pending.set(None),
                }
            },
            None => rsx! {},
        }
    }
}

#[component]
fn LoadingCard() -> Element {
    rsx! {
        div { class: "bg-white rounded-lg shadow p-6 text-center",
            span { class: "animate-spin inline-block rounded-full h-6 w-6 border-b-2 border-blue-600" }
        }
    }
}

#[component]
fn ErrorCard(message: String) -> Element {
    rsx! {
        div { class: "bg-red-50 border border-red-200 text-red-700 rounded-lg p-4 text-sm",
            {message}
        }
    }
}

fn source_badge(source: &str) -> (&'static str, String) {
    match source {
        "override" => (
            "bg-purple-100 text-purple-800",
            t!("system-retention-source-override"),
        ),
        "global" => (
            "bg-blue-100 text-blue-800",
            t!("system-retention-source-global"),
        ),
        "tier" => (
            "bg-green-100 text-green-800",
            t!("system-retention-source-tier"),
        ),
        _ => (
            "bg-gray-100 text-gray-800",
            t!("system-retention-source-env"),
        ),
    }
}

/// Parses a days input: empty = `None` (inherit), else 1..=3650.
pub fn parse_days(raw: &str) -> Result<Option<u32>, ()> {
    let raw = raw.trim();
    if raw.is_empty() {
        return Ok(None);
    }
    match raw.parse::<u32>() {
        Ok(d) if (1..=3650).contains(&d) => Ok(Some(d)),
        _ => Err(()),
    }
}

/// Effective retention of the current org — read-only here: the org
/// override and the platform default are set in Platform status (platform
/// admins), the subscription decides in SaaS.
#[component]
fn RetentionCard(info: RetentionInfo) -> Element {
    let (badge_class, badge_label) = source_badge(&info.source);
    let saas = info.deployment_mode == "saas";
    rsx! {
        section { class: "bg-white rounded-lg shadow p-6 space-y-4",
            div { class: "flex items-start justify-between gap-4 flex-wrap",
                div {
                    h2 { class: "text-lg font-semibold text-gray-900",
                        {t!("system-retention-title")}
                    }
                    p { class: "text-sm text-gray-600", {t!("system-retention-help")} }
                }
                span { class: "inline-flex items-center px-2.5 py-0.5 rounded-full text-xs font-medium {badge_class}",
                    {badge_label}
                }
            }
            div { class: "flex items-baseline gap-2",
                span { class: "text-4xl font-bold text-gray-900", "{info.days}" }
                span { class: "text-gray-600", {t!("system-days", count : info.days)} }
            }
            if info.clamped {
                p { class: "text-sm text-amber-700 bg-amber-50 border border-amber-200 rounded p-2",
                    {t!("system-retention-clamped")}
                }
            }
            // The subscription only drives retention in SaaS.
            if saas {
                if let Some(tier) = info.tier_name.clone() {
                    p { class: "text-sm text-gray-600",
                        {
                            t!(
                                "system-retention-tier", tier : tier, days : info.tier_days.map(| d | d
                                .to_string()).unwrap_or_else(|| "—".into())
                            )
                        }
                    }
                }
            }
            p { class: "text-sm text-gray-500",
                if saas {
                    {t!("system-retention-by-plan")}
                } else {
                    {t!("system-retention-set-by-admin")}
                }
                if info.editable {
                    " "
                    Link {
                        to: crate::app::Route::AdminStatus {},
                        class: "text-blue-600 hover:underline",
                        {t!("system-retention-change-in-admin")}
                    }
                }
            }
        }
    }
}

#[component]
fn DataCard(
    streams: Option<Result<pnex_core::O2StreamList, api::error::ApiError>>,
    can_write: bool,
    on_action: Callback<Pending>,
) -> Element {
    let (state, list) = match &streams {
        None => (None, None),
        Some(Ok(list)) => (Some(Ok(())), Some(list.clone())),
        Some(Err(err)) => (Some(Err(err.clone())), None),
    };
    let rows: Vec<O2StreamInfo> = list.as_ref().map(|l| l.streams.clone()).unwrap_or_default();
    let mut search = use_signal(String::new);
    let mut page = use_signal(|| 0i64);
    let mut selected = use_signal(Vec::<String>::new);

    // Client-side filter + pagination: O2 returns every stream at once.
    let term = search().trim().to_lowercase();
    let filtered: Vec<O2StreamInfo> = rows
        .iter()
        .filter(|s| term.is_empty() || s.name.to_lowercase().contains(&term))
        .cloned()
        .collect();
    let filtered_count = filtered.len() as i64;
    let last_page = ((filtered_count - 1).max(0)) / PAGE_SIZE;
    let page_idx = page().min(last_page);
    let page_rows: Vec<O2StreamInfo> = filtered
        .iter()
        .skip((page_idx * PAGE_SIZE) as usize)
        .take(PAGE_SIZE as usize)
        .cloned()
        .collect();
    // Selection pruned to streams still listed (a deleted stream drops out).
    let live_selection: Vec<String> = selected
        .read()
        .iter()
        .filter(|n| rows.iter().any(|s| &s.name == *n))
        .cloned()
        .collect();
    let filtered_names: Vec<String> = filtered.iter().map(|s| s.name.clone()).collect();
    let all_selected =
        !filtered_names.is_empty() && filtered_names.iter().all(|n| live_selection.contains(n));
    let selection_count = live_selection.len();
    // On-disk (compressed) size everywhere: what the quota and Platform
    // status count; the raw ingested size is only a tooltip.
    let total: f64 = rows
        .iter()
        .filter_map(|s| s.compressed_bytes)
        .map(|b| b as f64)
        .sum();
    // Subscription quota (SaaS only): (used, quota, over).
    let quota = list.as_ref().and_then(|l| {
        l.quota_bytes
            .map(|q| (l.used_bytes as f64, q as f64, l.over_quota))
    });
    let configured = list.as_ref().is_none_or(|l| l.configured);
    let provisioned = list.as_ref().is_none_or(|l| l.provisioned);
    let empty_message = if !configured {
        t!("system-data-not-configured").to_string()
    } else if !provisioned {
        t!("system-data-not-provisioned").to_string()
    } else {
        t!("system-data-empty").to_string()
    };

    let mut columns = Vec::new();
    if can_write {
        columns.push(
            Column::new(String::new(), move |s: &O2StreamInfo| {
                let name = s.name.clone();
                let stream_label = s.name.clone();
                let checked = selected.read().contains(&name);
                rsx! {
                    input {
                        class: "h-4 w-4 accent-blue-600",
                        r#type: "checkbox",
                        aria_label: "{stream_label}",
                        checked,
                        onchange: move |event: FormEvent| {
                            let on = event.checked();
                            let mut next = selected.peek().clone();
                            next.retain(|n| *n != name);
                            if on {
                                next.push(name.clone());
                            }
                            selected.set(next);
                        },
                    }
                }
            })
            .with_td_class("w-8"),
        );
    }
    columns.extend([
        Column::new(t!("system-col-stream").to_string(), |s: &O2StreamInfo| {
            rsx! {
                {s.name.clone()}
            }
        })
        .with_td_class("font-mono text-sm text-gray-900"),
        Column::new(t!("system-col-docs").to_string(), |s: &O2StreamInfo| {
            rsx! {
                {s.doc_num.map(|n| n.to_string()).unwrap_or_else(|| "—".into())}
            }
        })
        .with_td_class("text-gray-600")
        .secondary(),
        Column::new(t!("system-col-size").to_string(), |s: &O2StreamInfo| {
            let disk = s
                .compressed_bytes
                .map(|b| format_bytes(b as f64))
                .unwrap_or_else(|| "—".into());
            let raw = s
                .storage_bytes
                .map(|b| t!("system-size-raw", size: format_bytes(b as f64)).to_string())
                .unwrap_or_default();
            rsx! {
                span { title: "{raw}", {disk} }
            }
        })
        .with_td_class("text-gray-600"),
        Column::new(t!("system-col-period").to_string(), |s: &O2StreamInfo| {
            rsx! {
                {format!("{} → {}", short_utc(&s.time_min), short_utc(&s.time_max))}
            }
        })
        .with_td_class("text-gray-600 text-sm")
        .secondary(),
        Column::new(
            t!("system-col-retention").to_string(),
            |s: &O2StreamInfo| {
                if s.retention_days > 0 {
                    rsx! {
                        {t!("system-days-short", count : s.retention_days)}
                    }
                } else {
                    rsx! {
                        {t!("system-retention-o2-default")}
                    }
                }
            },
        )
        .with_td_class("text-gray-600"),
    ]);
    if can_write {
        columns.push(Column::new(t!("common-actions").to_string(), move |s: &O2StreamInfo| {
            let range_name = s.name.clone();
            let delete_name = s.name.clone();
            rsx! {
                div { class: "flex items-center gap-2",
                    button {
                        class: "px-3 py-1 text-sm whitespace-nowrap bg-gray-100 text-gray-700 rounded-lg hover:bg-gray-200",
                        onclick: move |_| on_action.call(Pending::DeleteRange(range_name.clone())),
                        {t!("system-delete-range")}
                    }
                    button {
                        class: "{DANGER_BTN} whitespace-nowrap",
                        onclick: move |_| on_action.call(Pending::DeleteStream(delete_name.clone())),
                        icons::Trash2 { class: "h-4 w-4 inline mr-1" }
                        {t!("common-delete")}
                    }
                }
            }
        }).actions());
    }

    rsx! {
        section { class: "bg-white rounded-lg shadow p-6 space-y-4",
            div { class: "flex items-start justify-between gap-4 flex-wrap",
                div {
                    h2 { class: "text-lg font-semibold text-gray-900", {t!("system-data-title")} }
                    p { class: "text-sm text-gray-600",
                        {t!("system-data-summary", count : rows.len(), size : format_bytes(total))}
                    }
                }
                if can_write && !rows.is_empty() {
                    button {
                        class: "inline-flex items-center px-4 py-2 text-sm font-semibold text-white bg-red-600 rounded-lg hover:bg-red-700",
                        onclick: move |_| on_action.call(Pending::Purge),
                        icons::Trash2 { class: "h-4 w-4 mr-2" }
                        {t!("system-purge")}
                    }
                }
            }
            if let Some((used, quota, over)) = quota {
                QuotaGauge { used, quota, over }
            }
            ListStates { state, is_empty: rows.is_empty(), empty_message,
                div { class: "flex flex-wrap items-center gap-3 mb-3",
                    input {
                        class: "flex-1 min-w-48 px-3 py-2 border border-gray-300 rounded-lg text-sm",
                        r#type: "search",
                        aria_label: t!("system-search-placeholder"),
                        placeholder: t!("system-search-placeholder"),
                        value: "{search}",
                        oninput: move |event| {
                            search.set(event.value());
                            page.set(0);
                        },
                    }
                    if can_write {
                        label { class: "flex items-center gap-2 text-sm text-gray-700 select-none",
                            input {
                                class: "h-4 w-4 accent-blue-600",
                                r#type: "checkbox",
                                checked: all_selected,
                                disabled: filtered_names.is_empty(),
                                onchange: {
                                    let names = filtered_names.clone();
                                    move |event: FormEvent| {
                                        let on = event.checked();
                                        let mut next = selected.peek().clone();
                                        next.retain(|n| !names.contains(n));
                                        if on {
                                            next.extend(names.iter().cloned());
                                        }
                                        selected.set(next);
                                    }
                                },
                            }
                            {t!("system-select-all", count : filtered_names.len())}
                        }
                        if selection_count > 0 {
                            button {
                                class: "{DANGER_BTN} whitespace-nowrap",
                                onclick: {
                                    let names = live_selection.clone();
                                    move |_| on_action.call(Pending::DeleteStreams(names.clone()))
                                },
                                icons::Trash2 { class: "h-4 w-4 inline mr-1" }
                                {t!("system-delete-selected", count : selection_count)}
                            }
                        }
                    }
                }
                DataTable {
                    columns,
                    rows: page_rows,
                    row_key: RowKey::new(|s: &O2StreamInfo| s.name.clone()),
                }
                ListPager { count: filtered_count, page }
            }
        }
    }
}

/// Telemetry storage against the subscription quota (informative).
#[component]
fn QuotaGauge(used: f64, quota: f64, over: bool) -> Element {
    let ratio = if quota > 0.0 {
        (used / quota).clamp(0.0, 1.0)
    } else {
        1.0
    };
    let bar_class = if over {
        "bg-red-500"
    } else if ratio > 0.8 {
        "bg-amber-500"
    } else {
        "bg-green-500"
    };
    rsx! {
        div { class: "space-y-1",
            div { class: "flex justify-between text-sm",
                span { class: "text-gray-600", {t!("system-quota-label")} }
                span { class: "font-medium text-gray-900",
                    {format!("{} / {}", format_bytes(used), format_bytes(quota))}
                }
            }
            div { class: "h-2 w-full bg-gray-100 rounded",
                div {
                    class: "h-2 rounded {bar_class}",
                    style: "width: {ratio * 100.0:.1}%",
                }
            }
            if over {
                p { class: "text-sm text-red-700", {t!("system-quota-over")} }
            }
        }
    }
}

#[component]
fn DeleteStreamDialog(name: String, on_done: Callback<()>, on_cancel: Callback<()>) -> Element {
    let stream = name.clone();
    rsx! {
        ConfirmDialog {
            title: t!("system-delete-stream-title").to_string(),
            message: t!("system-delete-stream-message", name : name.clone()).to_string(),
            confirm_label: t!("common-delete").to_string(),
            on_confirm: move |_| {
                let stream = stream.clone();
                spawn(async move {
                    match api::system::delete_stream(&stream).await {
                        Ok(_) => toasts::success("toast-system-deleted"),
                        Err(err) => toasts::error(err),
                    }
                    on_done.call(());
                });
            },
            on_cancel: move |_| on_cancel.call(()),
        }
    }
}

#[component]
fn DeleteStreamsDialog(
    names: Vec<String>,
    on_done: Callback<()>,
    on_cancel: Callback<()>,
) -> Element {
    let count = names.len();
    rsx! {
        ConfirmDialog {
            title: t!("system-delete-streams-title").to_string(),
            message: t!("system-delete-streams-message", count : count).to_string(),
            confirm_label: t!("common-delete").to_string(),
            on_confirm: move |_| {
                let names = names.clone();
                spawn(async move {
                    // Sequential: one O2 call per stream, the last error is reported.
                    let mut failure = None;
                    for stream in &names {
                        if let Err(err) = api::system::delete_stream(stream).await {
                            failure = Some(err);
                        }
                    }
                    match failure {
                        None => toasts::success("toast-system-deleted"),
                        Some(err) => toasts::error(err),
                    }
                    on_done.call(());
                });
            },
            on_cancel: move |_| on_cancel.call(()),
        }
    }
}

/// `datetime-local` value (`YYYY-MM-DDTHH:MM`, interpreted as UTC) → RFC 3339.
fn local_input_to_rfc3339(raw: &str) -> Option<String> {
    let raw = raw.trim();
    (raw.len() >= 16).then(|| format!("{}:00Z", &raw[..16]))
}

#[component]
fn DeleteRangeDialog(name: String, on_done: Callback<()>, on_cancel: Callback<()>) -> Element {
    let mut start = use_signal(String::new);
    let mut end = use_signal(String::new);
    let mut busy = use_signal(|| false);
    let stream = name.clone();
    let submit = move |_| {
        let (Some(s), Some(e)) = (
            local_input_to_rfc3339(&start()),
            local_input_to_rfc3339(&end()),
        ) else {
            toasts::error("err-o2-time-range-invalid");
            return;
        };
        let stream = stream.clone();
        busy.set(true);
        spawn(async move {
            match api::system::delete_range(&stream, s, e).await {
                Ok(_) => {
                    toasts::success("toast-system-range-scheduled");
                    on_done.call(());
                }
                Err(err) => toasts::error(err),
            }
            busy.set(false);
        });
    };
    rsx! {
        div {
            class: "fixed inset-0 bg-black/50 flex items-center justify-center z-50 p-4",
            onclick: move |_| on_cancel.call(()),
            div {
                class: "bg-white rounded-lg shadow-xl max-w-md w-full max-h-full overflow-y-auto p-4 space-y-4 sm:p-6",
                role: "dialog",
                aria_modal: "true",
                aria_label: t!("system-delete-range-title", name : name.clone()),
                onclick: move |e| e.stop_propagation(),
                h3 { class: "text-lg font-semibold text-gray-900",
                    {t!("system-delete-range-title", name : name.clone())}
                }
                p { class: "text-sm text-gray-600", {t!("system-delete-range-help")} }
                label {
                    r#for: "system-field-1",
                    class: "block text-sm font-medium text-gray-700",
                    {t!("system-range-start")}
                }
                input {
                    id: "system-field-1",
                    class: "w-full px-3 py-2 border border-gray-300 rounded-lg",
                    r#type: "datetime-local",
                    value: "{start}",
                    oninput: move |e| start.set(e.value()),
                }
                label {
                    r#for: "system-field-2",
                    class: "block text-sm font-medium text-gray-700",
                    {t!("system-range-end")}
                }
                input {
                    id: "system-field-2",
                    class: "w-full px-3 py-2 border border-gray-300 rounded-lg",
                    r#type: "datetime-local",
                    value: "{end}",
                    oninput: move |e| end.set(e.value()),
                }
                div { class: "flex justify-end gap-3 pt-2",
                    button {
                        class: "px-4 py-2 text-sm text-gray-600 hover:text-gray-900",
                        onclick: move |_| on_cancel.call(()),
                        {t!("common-cancel")}
                    }
                    button {
                        class: "px-4 py-2 text-sm font-semibold text-white bg-red-600 rounded-lg hover:bg-red-700 disabled:opacity-50",
                        disabled: busy(),
                        onclick: submit,
                        {t!("common-delete")}
                    }
                }
            }
        }
    }
}

#[component]
fn PurgeDialog(on_done: Callback<()>, on_cancel: Callback<()>) -> Element {
    let org_name = current_org_name();
    let mut typed = use_signal(String::new);
    let mut busy = use_signal(|| false);
    let matches = typed().trim() == org_name && !org_name.is_empty();
    let submit = move |_| {
        let confirm = typed().trim().to_string();
        busy.set(true);
        spawn(async move {
            match api::system::purge(confirm).await {
                Ok(_) => {
                    toasts::success("toast-system-deleted");
                    on_done.call(());
                }
                Err(err) => toasts::error(err),
            }
            busy.set(false);
        });
    };
    rsx! {
        div {
            class: "fixed inset-0 bg-black/50 flex items-center justify-center z-50 p-4",
            onclick: move |_| on_cancel.call(()),
            div {
                class: "bg-white rounded-lg shadow-xl max-w-md w-full max-h-full overflow-y-auto p-4 space-y-4 sm:p-6",
                role: "alertdialog",
                aria_modal: "true",
                aria_label: t!("system-purge-title"),
                onclick: move |e| e.stop_propagation(),
                h3 { class: "text-lg font-semibold text-red-700", {t!("system-purge-title")} }
                p { class: "text-sm text-gray-600",
                    {t!("system-purge-message", name : org_name.clone())}
                }
                input {
                    class: "w-full px-3 py-2 border border-gray-300 rounded-lg focus:ring-2 focus:ring-red-500",
                    r#type: "text",
                    placeholder: "{org_name}",
                    value: "{typed}",
                    oninput: move |e| typed.set(e.value()),
                }
                div { class: "flex justify-end gap-3 pt-2",
                    button {
                        class: "px-4 py-2 text-sm text-gray-600 hover:text-gray-900",
                        onclick: move |_| on_cancel.call(()),
                        {t!("common-cancel")}
                    }
                    button {
                        class: "px-4 py-2 text-sm font-semibold text-white bg-red-600 rounded-lg hover:bg-red-700 disabled:opacity-50",
                        disabled: !matches || busy(),
                        onclick: submit,
                        {t!("system-purge")}
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
    fn days_input_parsing() {
        assert_eq!(parse_days(""), Ok(None));
        assert_eq!(parse_days(" 30 "), Ok(Some(30)));
        assert!(parse_days("0").is_err());
        assert!(parse_days("abc").is_err());
    }

    #[test]
    fn datetime_local_becomes_utc_rfc3339() {
        assert_eq!(
            local_input_to_rfc3339("2026-09-28T10:17"),
            Some("2026-09-28T10:17:00Z".to_string())
        );
        assert_eq!(local_input_to_rfc3339(""), None);
    }

    #[test]
    fn bytes_are_humanized() {
        assert_eq!(format_bytes(512.0), "512 B");
        assert_eq!(format_bytes(1536.0), "1.5 KB");
    }
}
