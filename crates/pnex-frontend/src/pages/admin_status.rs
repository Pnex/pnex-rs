//! Platform status page (D72, platform admin only) — one card per infra
//! component (database, Rauthy, OpenObserve, Valkey, object and local
//! storage, host machine, AI service, secrets vault; auto-refresh 30 s),
//! then the platform settings: organizations (platform default retention,
//! per-org retention, on-disk storage) and master key rotation.
//!
//! Metric/component keys come from the server and are resolved through the
//! non-panicking `error_i18n::resolve` (unknown key → raw key, never a
//! panic); details are runtime diagnostics displayed verbatim.

use std::time::Duration;

use dioxus::prelude::*;
use dioxus_i18n::t;
use pnex_core::{ComponentStatus, OrgSystemRow, StatusMetric};

use crate::api;
use crate::api::error_i18n::resolve;
use crate::components::crud::filters::RefreshButton;
use crate::components::crud::layout::ListLayout;
use crate::pages::system::{format_bytes, parse_days};
use crate::state::{session, toasts};

const AUTO_REFRESH: Duration = Duration::from_secs(30);

#[component]
pub fn AdminStatus() -> Element {
    let is_admin = session::user().is_some_and(|u| u.platform_admin);
    let mut reload = use_signal(|| 0u32);

    let status = use_resource(move || async move {
        let _ = reload();
        api::system::status().await
    });

    // Auto-refresh loop, dropped with the page.
    use_future(move || async move {
        loop {
            crate::util::sleep(AUTO_REFRESH).await;
            reload.with_mut(|r| *r += 1);
        }
    });

    if !is_admin {
        return rsx! {
            div { class: "p-6",
                div { class: "bg-amber-50 border border-amber-200 text-amber-800 rounded-lg p-4 text-sm",
                    {t!("err-platform-admin-required")}
                }
            }
        };
    }

    let subtitle = match &*status.value().read() {
        Some(Ok(s)) => t!(
            "admin-status-subtitle",
            version: s.server_version.clone(),
            mode: resolve(&format!("system-mode-{}", s.deployment_mode.replace('_', "-")), None),
            at: s.generated_at.get(..19).unwrap_or(&s.generated_at).replace('T', " ")
        )
        .to_string(),
        _ => t!("admin-status-subtitle-loading").to_string(),
    };

    rsx! {
        ListLayout {
            title: t!("admin-status-title").to_string(),
            subtitle: Some(subtitle),
            can_write: false,
            actions: rsx! {
                RefreshButton { on_click: move |_| reload.with_mut(|r| *r += 1) }
            },
            match &*status.value().read() {
                None => rsx! {
                    div { class: "text-center py-12",
                        span { class: "animate-spin inline-block rounded-full h-8 w-8 border-b-2 border-blue-600" }
                    }
                },
                Some(Err(err)) => rsx! {
                    div { class: "bg-red-50 border border-red-200 text-red-700 rounded-lg p-4 text-sm",
                        {api::error_i18n::localize(err)}
                    }
                },
                Some(Ok(report)) => rsx! {
                    div { class: "grid gap-4 md:grid-cols-2 xl:grid-cols-3",
                        for comp in report.components.clone() {
                            ComponentCard { key: "{comp.key}", comp }
                        }
                    }
                },
            }
            OrgsOverview {}
            div { class: "mt-8",
                crate::components::ai_retention::AiRetentionCard { platform: true }
            }
            section { class: "mt-8 bg-white rounded-lg shadow p-5",
                SecretsRekey {
                    stale: stale_secrets(&status.value().read()),
                    on_done: move |_| reload.with_mut(|r| *r += 1),
                }
            }
        }
    }
}

/// `secrets_stale` metric of the `secrets` component, when reported.
fn stale_secrets(
    report: &Option<Result<pnex_core::SystemStatus, api::error::ApiError>>,
) -> Option<u64> {
    let Some(Ok(report)) = report else {
        return None;
    };
    report
        .components
        .iter()
        .find(|c| c.key == "secrets")?
        .metrics
        .iter()
        .find(|m| m.key == "secrets_stale")?
        .value
        .map(|v| v as u64)
}

/// Master key rotation (secrets.md S8): re-encrypts every vault secret
/// with the write key, once every pod runs with the new keyring.
#[component]
fn SecretsRekey(stale: Option<u64>, on_done: EventHandler<()>) -> Element {
    let mut confirming = use_signal(|| false);
    let mut busy = use_signal(|| false);
    rsx! {
        div { class: "space-y-2",
            h3 { class: "text-sm font-semibold text-gray-900", {t!("admin-rekey-title")} }
            p { class: "text-xs text-gray-500", {t!("admin-rekey-help")} }
            if let Some(n) = stale {
                p { class: "text-sm text-gray-700", {t!("admin-rekey-stale", count : n)} }
            }
            div { class: "flex gap-2",
                if confirming() {
                    button {
                        r#type: "button",
                        class: "px-3 py-1.5 text-sm rounded-lg bg-red-600 text-white hover:bg-red-700",
                        disabled: busy(),
                        onclick: move |_| {
                            busy.set(true);
                            spawn(async move {
                                match api::system::rekey_secrets().await {
                                    Ok(r) => {
                                        toasts::success(
                                            t!(
                                                "admin-rekey-done", rewritten : r.rewritten, unreadable : r
                                                .unreadable, remaining : r.remaining
                                            )
                                                .to_string(),
                                        )
                                    }
                                    Err(err) => toasts::error(err),
                                }
                                busy.set(false);
                                confirming.set(false);
                                on_done.call(());
                            });
                        },
                        {t!("admin-rekey-confirm")}
                    }
                    button {
                        r#type: "button",
                        class: "px-3 py-1.5 text-sm border border-gray-300 rounded-lg hover:bg-gray-50",
                        onclick: move |_| confirming.set(false),
                        {t!("llm-cancel")}
                    }
                } else {
                    button {
                        r#type: "button",
                        class: "px-3 py-1.5 text-sm border border-gray-300 rounded-lg hover:bg-gray-50",
                        disabled: stale == Some(0),
                        onclick: move |_| confirming.set(true),
                        {t!("admin-rekey-run")}
                    }
                }
            }
        }
    }
}

/// Status pill — full literal classes (Tailwind scan).
fn status_pill(status: &str) -> (&'static str, &'static str) {
    match status {
        "ok" => ("bg-green-100 text-green-800", "bg-green-500"),
        "degraded" => ("bg-amber-100 text-amber-800", "bg-amber-500"),
        "down" => ("bg-red-100 text-red-800", "bg-red-500"),
        _ => ("bg-gray-100 text-gray-600", "bg-gray-400"),
    }
}

fn format_duration(secs: f64) -> String {
    let secs = secs.max(0.0) as u64;
    let (d, h, m) = (secs / 86_400, (secs % 86_400) / 3600, (secs % 3600) / 60);
    if d > 0 {
        format!("{d}d {h}h")
    } else if h > 0 {
        format!("{h}h {m}m")
    } else {
        format!("{m}m")
    }
}

/// Formatted value of a metric (text metrics may carry machine tokens).
fn metric_value(m: &StatusMetric) -> String {
    match m.unit.as_str() {
        "text" => {
            let text = m.text.clone().unwrap_or_default();
            if m.key == "machine_kind" {
                resolve(&format!("system-machine-{}", text.replace('_', "-")), None)
            } else {
                text
            }
        }
        _ => {
            let Some(v) = m.value else {
                return "—".into();
            };
            match m.unit.as_str() {
                "bytes" => match m.total {
                    Some(total) => format!("{} / {}", format_bytes(v), format_bytes(total)),
                    None => format_bytes(v),
                },
                "seconds" => format_duration(v),
                "percent" => format!("{v:.0} %"),
                _ => {
                    if v.fract() == 0.0 {
                        format!("{v:.0}")
                    } else {
                        format!("{v:.2}")
                    }
                }
            }
        }
    }
}

#[component]
fn ComponentCard(comp: ComponentStatus) -> Element {
    let (pill_class, dot_class) = status_pill(&comp.status);
    let title = resolve(
        &format!("system-component-{}", comp.key.replace('_', "-")),
        None,
    );
    let status_label = resolve(
        &format!("system-status-{}", comp.status.replace('_', "-")),
        None,
    );
    rsx! {
        section { class: "bg-white rounded-lg shadow p-5 space-y-3",
            div { class: "flex items-center justify-between gap-2",
                h2 { class: "font-semibold text-gray-900", {title} }
                span { class: "inline-flex items-center gap-1.5 px-2.5 py-0.5 rounded-full text-xs font-medium {pill_class}",
                    span { class: "h-2 w-2 rounded-full {dot_class}" }
                    {status_label}
                }
            }
            if let Some(ms) = comp.latency_ms {
                p { class: "text-xs text-gray-500", {t!("admin-status-latency", ms : ms)} }
            }
            if let Some(detail) = comp.detail.clone() {
                p { class: "text-xs text-gray-600 bg-gray-50 rounded p-2 font-mono break-all",
                    {detail}
                }
            }
            dl { class: "space-y-2",
                for (i, m) in comp.metrics.iter().cloned().enumerate() {
                    MetricRow { key: "{i}", metric: m }
                }
            }
        }
    }
}

#[component]
fn MetricRow(metric: StatusMetric) -> Element {
    let label = resolve(
        &format!("system-metric-{}", metric.key.replace('_', "-")),
        None,
    );
    let value = metric_value(&metric);
    // Gauge for bounded byte metrics (disk, memory).
    let ratio = match (metric.value, metric.total) {
        (Some(v), Some(total)) if total > 0.0 => Some((v / total).clamp(0.0, 1.0)),
        _ => None,
    };
    let bar_class = match ratio {
        Some(r) if r > 0.9 => "bg-red-500",
        Some(r) if r > 0.75 => "bg-amber-500",
        _ => "bg-blue-500",
    };
    // dt + dd (+ a gauge dd) in one group: the only children a <dl>
    // group may hold.
    rsx! {
        div { class: "flex flex-wrap justify-between gap-x-3 text-sm",
            dt { class: "text-gray-500",
                {label}
                if let Some(qualifier) = metric.label.clone() {
                    span { class: "ml-1 text-gray-400 font-mono text-xs break-all", "({qualifier})" }
                }
            }
            dd { class: "text-gray-900 font-medium text-right", {value} }
            if let Some(r) = ratio {
                dd { class: "basis-full mt-1 h-1.5 bg-gray-100 rounded",
                    div {
                        class: "h-1.5 rounded {bar_class}",
                        style: "width: {r * 100.0:.1}%",
                    }
                }
            }
        }
    }
}

/// Every organization of the instance: platform default retention
/// (self-hosted), effective retention with inline override edit, on-disk
/// O2 usage and subscription quota.
#[component]
fn OrgsOverview() -> Element {
    let mut reload = use_signal(|| 0u32);
    let rows = use_resource(move || async move {
        let _ = reload();
        api::system::orgs_overview().await
    });
    // Platform default + deployment mode (carried by the retention info).
    let retention = use_resource(move || async move {
        let _ = reload();
        api::system::retention().await
    });
    let platform_default = match &*retention.value().read() {
        Some(Ok(info)) if info.deployment_mode != "saas" => Some(info.global_default_days),
        _ => None,
    };
    rsx! {
        section { class: "mt-8 bg-white rounded-lg shadow p-5 space-y-4",
            div {
                h2 { class: "text-lg font-semibold text-gray-900", {t!("admin-orgs-title")} }
                p { class: "text-sm text-gray-600", {t!("admin-orgs-help")} }
            }
            if let Some(days) = platform_default {
                PlatformDefaultRetention {
                    key: "{days:?}",
                    days,
                    on_saved: move |_| reload.with_mut(|r| *r += 1),
                }
            }
            match &*rows.value().read() {
                None => rsx! {
                    div { class: "text-center py-6",
                        span { class: "animate-spin inline-block rounded-full h-6 w-6 border-b-2 border-blue-600" }
                    }
                },
                Some(Err(err)) => rsx! {
                    p { class: "text-sm text-red-700", {api::error_i18n::localize(err)} }
                },
                Some(Ok(list)) => rsx! {
                    div { class: "overflow-x-auto",
                        table { class: "min-w-full text-sm",
                            thead {
                                tr { class: "text-left text-xs uppercase text-gray-500 border-b",
                                    th { class: "py-2 pr-4", {t!("admin-orgs-col-org")} }
                                    th { class: "py-2 pr-4", {t!("admin-orgs-col-tier")} }
                                    th { class: "py-2 pr-4", {t!("admin-orgs-col-retention")} }
                                    th { class: "py-2 pr-4", {t!("admin-orgs-col-override")} }
                                    th { class: "py-2 pr-4", {t!("admin-orgs-col-usage")} }
                                }
                            }
                            tbody {
                                for row in list.clone() {
                                    OrgRow {
                                        key: "{row.org_id}",
                                        row,
                                        on_saved: move |_| reload.with_mut(|r| *r += 1),
                                    }
                                }
                            }
                        }
                    }
                },
            }
        }
    }
}

/// Retention applied to every organization without a specific value
/// (self-hosted only; SaaS follows the subscription).
#[component]
fn PlatformDefaultRetention(days: Option<u32>, on_saved: Callback<()>) -> Element {
    let mut input = use_signal(|| days.map(|d| d.to_string()).unwrap_or_default());
    let mut busy = use_signal(|| false);
    let mut save = move |raw: String| {
        let Ok(days) = parse_days(&raw) else {
            toasts::error("err-retention-out-of-range");
            return;
        };
        busy.set(true);
        spawn(async move {
            match api::system::set_default_retention(days).await {
                Ok(_) => {
                    toasts::success("toast-saved");
                    on_saved.call(());
                }
                Err(err) => toasts::error(err),
            }
            busy.set(false);
        });
    };
    rsx! {
        div { class: "space-y-2 pb-4 border-b border-gray-100",
            label { class: "block text-sm font-medium text-gray-700", {t!("system-retention-global")} }
            div { class: "flex gap-2",
                input {
                    class: "w-24 px-2 py-1 border border-gray-300 rounded",
                    r#type: "number",
                    min: "1",
                    max: "3650",
                    placeholder: t!("system-retention-inherit"),
                    value: "{input}",
                    oninput: move |e| input.set(e.value()),
                }
                button {
                    class: "px-2 py-1 text-xs bg-blue-600 text-white rounded hover:bg-blue-700 disabled:opacity-50",
                    disabled: busy(),
                    onclick: move |_| save(input()),
                    {t!("common-save")}
                }
                button {
                    class: "px-2 py-1 text-xs text-gray-600 border border-gray-300 rounded hover:bg-gray-50 disabled:opacity-50",
                    disabled: busy(),
                    onclick: move |_| {
                        input.set(String::new());
                        save(String::new());
                    },
                    {t!("system-retention-reset")}
                }
            }
            p { class: "text-xs text-gray-500", {t!("system-retention-global-help")} }
        }
    }
}

#[component]
fn OrgRow(row: OrgSystemRow, on_saved: Callback<()>) -> Element {
    let mut input = use_signal(|| {
        row.org_override_days
            .map(|d| d.to_string())
            .unwrap_or_default()
    });
    let mut busy = use_signal(|| false);
    let org_id = row.org_id;
    let mut save = move |raw: String| {
        let Ok(days) = parse_days(&raw) else {
            toasts::error("err-retention-out-of-range");
            return;
        };
        busy.set(true);
        spawn(async move {
            match api::system::set_org_retention(org_id, days).await {
                Ok(_) => {
                    toasts::success("toast-saved");
                    on_saved.call(());
                }
                Err(err) => toasts::error(err),
            }
            busy.set(false);
        });
    };
    let source = resolve(
        &format!("system-retention-source-{}", row.retention_source),
        None,
    );
    let usage = match (row.provisioned, row.used_bytes) {
        (false, _) => t!("admin-orgs-no-data").to_string(),
        (true, None) => "—".to_string(),
        (true, Some(used)) => match row.quota_bytes {
            Some(q) => format!("{} / {}", format_bytes(used as f64), format_bytes(q as f64)),
            None => format_bytes(used as f64),
        },
    };
    let over = matches!((row.used_bytes, row.quota_bytes), (Some(u), Some(q)) if u > q);
    rsx! {
        tr { class: "border-b last:border-0",
            td { class: "py-2 pr-4 font-medium text-gray-900", {row.name.clone()} }
            td { class: "py-2 pr-4 text-gray-600",
                {row.tier_name.clone().unwrap_or_else(|| "—".into())}
            }
            td { class: "py-2 pr-4 text-gray-900",
                {t!("system-days-short", count : row.retention_days)}
                span { class: "ml-2 text-xs text-gray-500", "({source})" }
            }
            td { class: "py-2 pr-4",
                div { class: "flex gap-2",
                    input {
                        class: "w-24 px-2 py-1 border border-gray-300 rounded",
                        r#type: "number",
                        min: "1",
                        max: "3650",
                        placeholder: t!("system-retention-inherit"),
                        value: "{input}",
                        oninput: move |e| input.set(e.value()),
                    }
                    button {
                        class: "px-2 py-1 text-xs bg-blue-600 text-white rounded hover:bg-blue-700 disabled:opacity-50",
                        disabled: busy(),
                        onclick: move |_| save(input()),
                        {t!("common-save")}
                    }
                    button {
                        class: "px-2 py-1 text-xs text-gray-600 border border-gray-300 rounded hover:bg-gray-50 disabled:opacity-50",
                        disabled: busy(),
                        onclick: move |_| {
                            input.set(String::new());
                            save(String::new());
                        },
                        {t!("system-retention-reset")}
                    }
                }
            }
            td { class: if over { "py-2 pr-4 text-red-700 font-medium" } else { "py-2 pr-4 text-gray-600" },
                {usage}
                if let Some(n) = row.stream_count {
                    span { class: "ml-2 text-xs text-gray-400", {t!("admin-orgs-streams", count : n)} }
                }
            }
        }
    }
}
