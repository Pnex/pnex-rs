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
use pnex_core::{ComponentStatus, MetricUnit, OrgSystemRow, StatusMetric, TierOption};

use crate::api;
use crate::api::error_i18n::resolve;
use crate::components::crud::layout::ListLayout;
use crate::components::gerbe::Gerbe;
use crate::components::ui::{BTN_DANGER_SOLID, BTN_PRIMARY_SM, BTN_SECONDARY_SM};
use crate::components::ui::{FIELD_ERROR, FIELD_HELP, FIELD_LABEL, INPUT};
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
            mode: resolve(&format!("system-mode-{}", s.deployment_mode.as_str().replace('_', "-")), None),
            at: s.generated_at.get(..19).unwrap_or(&s.generated_at).replace('T', " ")
        )
        .to_string(),
        _ => t!("admin-status-subtitle-loading").to_string(),
    };

    rsx! {
        ListLayout {
            title: t!("admin-status-title").to_string(),
            subtitle: Some(subtitle),
            on_refresh: move |_| reload.with_mut(|r| *r += 1),
            can_write: false,
            match &*status.value().read() {
                None => rsx! {
                    div { class: "text-center py-12",
                        Gerbe { size: 36 }
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
            AgentLimitsCard {}
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
                        class: BTN_DANGER_SOLID,
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
    match m.unit {
        MetricUnit::Text => {
            let text = m.text.clone().unwrap_or_default();
            match m.key.as_str() {
                "machine_kind" => {
                    resolve(&format!("system-machine-{}", text.replace('_', "-")), None)
                }
                "cluster_runtime" | "cluster_mode" | "cluster_node" => {
                    resolve(&format!("system-cluster-{}", text.replace('_', "-")), None)
                }
                _ => text,
            }
        }
        _ => {
            let Some(v) = m.value else {
                return "—".into();
            };
            match m.unit {
                MetricUnit::Bytes => match m.total {
                    Some(total) => format!("{} / {}", format_bytes(v), format_bytes(total)),
                    None => format_bytes(v),
                },
                MetricUnit::Seconds => format_duration(v),
                MetricUnit::Percent => format!("{v:.0} %"),
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
    let (pill_class, dot_class) = status_pill(comp.status.as_str());
    let title = resolve(
        &format!("system-component-{}", comp.key.replace('_', "-")),
        None,
    );
    let status_label = resolve(
        &format!("system-status-{}", comp.status.as_str().replace('_', "-")),
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
        _ => "bg-signal-500",
    };
    // dt + dd (+ a gauge dd) in one group: the only children a <dl>
    // group may hold.
    rsx! {
        div { class: "flex flex-wrap justify-between gap-x-3 text-sm",
            dt { class: "text-gray-500",
                {label}
                if let Some(qualifier) = metric.label.clone() {
                    span { class: "ml-1 text-gray-500 font-mono text-xs break-all", "({qualifier})" }
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
    let tiers = use_resource(move || async move { api::system::tiers().await });
    let tier_options = match &*tiers.value().read() {
        Some(Ok(list)) => list.clone(),
        _ => Vec::new(),
    };
    let platform_default = match &*retention.value().read() {
        Some(Ok(info)) if info.deployment_mode != pnex_core::DeploymentMode::Saas => {
            Some(info.global_default_days)
        }
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
                        Gerbe { size: 28 }
                    }
                },
                Some(Err(err)) => rsx! {
                    p { class: "text-sm text-red-700", {api::error_i18n::localize(err)} }
                },
                Some(Ok(list)) => rsx! {
                    div { class: "overflow-x-auto",
                        table { class: "min-w-full text-sm",
                            thead {
                                tr { class: "text-left text-xs text-gray-500 border-b",
                                    th { class: "py-2 pr-4", {t!("admin-orgs-col-org")} }
                                    th { class: "py-2 pr-4", {t!("admin-orgs-col-tier")} }
                                    th { class: "py-2 pr-4", {t!("admin-orgs-col-retention")} }
                                    th { class: "py-2 pr-4", {t!("admin-orgs-col-override")} }
                                    th { class: "py-2 pr-4", {t!("admin-orgs-col-usage")} }
                                    th { class: "py-2 pr-4", {t!("admin-orgs-col-tables")} }
                                }
                            }
                            tbody {
                                for row in list.clone() {
                                    OrgRow {
                                        key: "{row.org_id}",
                                        row,
                                        tiers: tier_options.clone(),
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
            label { r#for: "admin-retention-global", class: FIELD_LABEL,
                {t!("system-retention-global")}
            }
            div { class: "flex gap-2",
                input {
                    id: "admin-retention-global",
                    class: "w-24 px-2 py-1 border border-gray-300 rounded",
                    r#type: "number",
                    min: "1",
                    max: "3650",
                    placeholder: t!("system-retention-inherit"),
                    value: "{input}",
                    oninput: move |e| input.set(e.value()),
                }
                button {
                    class: BTN_PRIMARY_SM,
                    disabled: busy(),
                    onclick: move |_| save(input()),
                    {t!("common-save")}
                }
                button {
                    class: BTN_SECONDARY_SM,
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
fn OrgRow(row: OrgSystemRow, tiers: Vec<TierOption>, on_saved: Callback<()>) -> Element {
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
    let mut save_tier = move |raw: String| {
        let tier_id = raw.parse::<i64>().ok();
        busy.set(true);
        spawn(async move {
            match api::system::set_org_tier(org_id, tier_id).await {
                Ok(_) => {
                    toasts::success("toast-saved");
                    on_saved.call(());
                }
                Err(err) => toasts::error(err),
            }
            busy.set(false);
        });
    };
    let tier_label = t!("admin-orgs-tier-label", org : row.name.clone());
    let override_label = t!("admin-orgs-override-label", org : row.name.clone());
    let source = resolve(
        &format!("system-retention-source-{}", row.retention_source.as_str()),
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
            td { class: "py-2 pr-4",
                select {
                    class: "px-2 py-1 border border-gray-300 rounded bg-white text-gray-700",
                    aria_label: "{tier_label}",
                    disabled: busy(),
                    onchange: move |e| save_tier(e.value()),
                    option { value: "", selected: row.tier_id.is_none(), {t!("admin-orgs-tier-none")} }
                    for tier in tiers.iter() {
                        option {
                            key: "{tier.id}",
                            value: "{tier.id}",
                            selected: row.tier_id == Some(tier.id),
                            {tier.name.clone()}
                        }
                    }
                }
            }
            td { class: "py-2 pr-4 text-gray-900 whitespace-nowrap",
                {t!("system-days-short", count : row.retention_days)}
                span { class: "ml-2 text-xs text-gray-500", "({source})" }
            }
            td { class: "py-2 pr-4",
                div { class: "flex gap-2",
                    input {
                        class: "w-24 px-2 py-1 border border-gray-300 rounded",
                        aria_label: "{override_label}",
                        r#type: "number",
                        min: "1",
                        max: "3650",
                        placeholder: t!("system-retention-inherit"),
                        value: "{input}",
                        oninput: move |e| input.set(e.value()),
                    }
                    button {
                        class: BTN_PRIMARY_SM,
                        disabled: busy(),
                        onclick: move |_| save(input()),
                        {t!("common-save")}
                    }
                    button {
                        class: BTN_SECONDARY_SM,
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
                    span { class: "ml-2 text-xs text-gray-500", {t!("admin-orgs-streams", count : n)} }
                }
            }
            td { class: "py-2 pr-4",
                TableQuotaCell { org_id: row.org_id, org: row.name.clone() }
            }
        }
    }
}

/// A table quota typed in GB (1024³ bytes, as `format_bytes` shows it);
/// empty returns to the default.
pub fn parse_gb(raw: &str) -> Result<Option<u64>, ()> {
    let raw = raw.trim();
    if raw.is_empty() {
        return Ok(None);
    }
    let gb: f64 = raw.replace(',', ".").parse().map_err(|_| ())?;
    if !gb.is_finite() || gb <= 0.0 || gb > 1_000_000.0 {
        return Err(());
    }
    Ok(Some((gb * 1024f64.powi(3)).round() as u64))
}

/// The table storage of an org and its quota (D821): bulk loads (AIS,
/// aircraft, DVF) only run once a platform admin has set it (SEC-82).
#[component]
fn TableQuotaCell(org_id: i64, org: String) -> Element {
    let mut reload = use_signal(|| 0u32);
    let quota = use_resource(move || async move {
        let _ = reload();
        api::system::table_quota(org_id).await
    });
    let mut input = use_signal(String::new);
    let mut busy = use_signal(|| false);
    let mut save = move |raw: String| {
        let Ok(bytes) = parse_gb(&raw) else {
            toasts::error("admin-orgs-tables-invalid");
            return;
        };
        busy.set(true);
        spawn(async move {
            match api::system::set_table_quota(org_id, bytes).await {
                Ok(_) => {
                    toasts::success("toast-saved");
                    input.set(String::new());
                    reload.with_mut(|r| *r += 1);
                }
                Err(err) => toasts::error(err),
            }
            busy.set(false);
        });
    };
    let label = t!("admin-orgs-tables-label", org : org.clone());
    let (usage, explicit, over) = match &*quota.value().read() {
        Some(Ok(q)) => {
            let limit = q.bytes.unwrap_or(q.default_bytes);
            (
                format!(
                    "{} / {}",
                    format_bytes(q.used_bytes as f64),
                    format_bytes(limit as f64)
                ),
                q.bytes.is_some(),
                q.used_bytes > limit,
            )
        }
        Some(Err(_)) => ("—".to_string(), false, false),
        None => ("…".to_string(), false, false),
    };
    rsx! {
        div { class: "flex flex-col gap-1",
            span { class: if over { "text-red-700 font-medium whitespace-nowrap" } else { "text-gray-700 whitespace-nowrap" },
                {usage}
                if !explicit {
                    span { class: "ml-2 text-xs text-gray-500", {t!("admin-orgs-tables-default")} }
                }
            }
            div { class: "flex gap-2",
                input {
                    class: "w-24 px-2 py-1 border border-gray-300 rounded",
                    aria_label: "{label}",
                    r#type: "number",
                    min: "1",
                    step: "1",
                    placeholder: t!("admin-orgs-tables-placeholder"),
                    value: "{input}",
                    oninput: move |e| input.set(e.value()),
                }
                button {
                    class: BTN_PRIMARY_SM,
                    disabled: busy(),
                    onclick: move |_| save(input()),
                    {t!("common-save")}
                }
                button {
                    class: BTN_SECONDARY_SM,
                    disabled: busy() || !explicit,
                    onclick: move |_| save(String::new()),
                    {t!("system-retention-reset")}
                }
            }
        }
    }
}

/// How a limit of hosted agents is typed: a Kubernetes quantity, days, or bytes in MiB.
#[derive(Clone, Copy, PartialEq)]
enum LimitUnit {
    Quantity,
    Days,
    Mib,
}

/// Fields of `AgentLimits`, in the order of the form.
const AGENT_LIMIT_FIELDS: [(&str, LimitUnit); 10] = [
    ("build_memory", LimitUnit::Quantity),
    ("build_ephemeral_storage", LimitUnit::Quantity),
    ("artifact_max_bytes", LimitUnit::Mib),
    ("artifact_run_max_bytes", LimitUnit::Mib),
    ("artifact_expires_days", LimitUnit::Days),
    ("depcache_max_bytes", LimitUnit::Mib),
    ("depcache_expires_days", LimitUnit::Days),
    ("sbom_days", LimitUnit::Days),
    ("scan_events_days", LimitUnit::Days),
    ("job_logs_days", LimitUnit::Days),
];

const MIB: f64 = 1_048_576.0;

fn limit_label(field: &str) -> String {
    match field {
        "build_memory" => t!("system-agents-build-memory"),
        "build_ephemeral_storage" => t!("system-agents-build-ephemeral-storage"),
        "artifact_max_bytes" => t!("system-agents-artifact-max-bytes"),
        "artifact_run_max_bytes" => t!("system-agents-artifact-run-max-bytes"),
        "artifact_expires_days" => t!("system-agents-artifact-expires-days"),
        "depcache_max_bytes" => t!("system-agents-depcache-max-bytes"),
        "depcache_expires_days" => t!("system-agents-depcache-expires-days"),
        "sbom_days" => t!("system-agents-sbom-days"),
        "scan_events_days" => t!("system-agents-scan-events-days"),
        "job_logs_days" => t!("system-agents-job-logs-days"),
        other => other.to_string(),
    }
    .to_string()
}

/// Form text of a limit: bytes in MiB, the rest as they are.
fn limit_text(limits: &serde_json::Value, field: &str, unit: LimitUnit) -> String {
    let v = &limits[field];
    match unit {
        LimitUnit::Quantity => v.as_str().unwrap_or_default().to_string(),
        LimitUnit::Days => v.to_string(),
        LimitUnit::Mib => v
            .as_u64()
            .map(|b| (b as f64 / MIB).to_string())
            .unwrap_or_default(),
    }
}

/// The limits typed in the form; `Err` names the first field that does not read.
fn limits_of(texts: &[String]) -> Result<pnex_core::environments::AgentLimits, &'static str> {
    let mut obj = serde_json::Map::new();
    for ((field, unit), raw) in AGENT_LIMIT_FIELDS.iter().zip(texts) {
        let raw = raw.trim();
        let value = match unit {
            LimitUnit::Quantity => serde_json::Value::from(raw),
            LimitUnit::Days => raw.parse::<u32>().map_err(|_| *field)?.into(),
            LimitUnit::Mib => {
                let mib = raw
                    .parse::<f64>()
                    .ok()
                    .filter(|m| m.is_finite() && *m > 0.0)
                    .ok_or(*field)?;
                ((mib * MIB).round() as u64).into()
            }
        };
        obj.insert(field.to_string(), value);
    }
    serde_json::from_value(obj.into()).map_err(|_| "body")
}

/// Global limits of hosted agents (D755, R9), for every organization.
#[component]
fn AgentLimitsCard() -> Element {
    let mut reload = use_signal(|| 0u32);
    let info = use_resource(move || async move {
        let _ = reload();
        api::system::agent_limits().await
    });
    rsx! {
        section { class: "mt-8 bg-white rounded-lg shadow p-5 space-y-4",
            div {
                h2 { class: "text-lg font-semibold text-gray-900", {t!("system-agents-title")} }
                p { class: "text-sm text-gray-600", {t!("system-agents-help")} }
            }
            match &*info.value().read() {
                None => rsx! {
                    div { class: "text-center py-6",
                        Gerbe { size: 28 }
                    }
                },
                Some(Err(err)) => rsx! {
                    p { class: "text-sm text-red-700", {api::error_i18n::localize(err)} }
                },
                Some(Ok(info)) => rsx! {
                    AgentLimitsForm {
                        key: "{reload}",
                        info: info.clone(),
                        on_saved: move |_| reload.with_mut(|r| *r += 1),
                    }
                },
            }
        }
    }
}

#[component]
fn AgentLimitsForm(
    info: pnex_core::environments::AgentLimitsInfo,
    on_saved: Callback<()>,
) -> Element {
    let current = serde_json::to_value(&info.limits).unwrap_or_default();
    let defaults = serde_json::to_value(&info.defaults).unwrap_or_default();
    let texts_of = |limits: &serde_json::Value| -> Vec<String> {
        AGENT_LIMIT_FIELDS
            .iter()
            .map(|(f, u)| limit_text(limits, f, *u))
            .collect()
    };
    let mut texts = use_signal(|| texts_of(&current));
    let default_texts = texts_of(&defaults);
    let mut errors = use_signal(serde_json::Map::<String, serde_json::Value>::new);
    let mut busy = use_signal(|| false);
    let save = move |_| {
        let limits = match limits_of(&texts.read()) {
            Ok(l) => l,
            Err(field) => {
                let mut one = serde_json::Map::new();
                one.insert(field.to_string(), "invalid".into());
                errors.set(one);
                return;
            }
        };
        errors.set(serde_json::Map::new());
        busy.set(true);
        spawn(async move {
            let saved = api::system::set_agent_limits(&limits).await;
            busy.set(false);
            match saved {
                Ok(_) => {
                    toasts::success("toast-saved");
                    on_saved.call(());
                }
                Err(err) => {
                    if let Some(fields) = err.fields() {
                        errors.set(fields.clone());
                    }
                    toasts::error(err);
                }
            }
        });
    };
    let rows: Vec<_> = AGENT_LIMIT_FIELDS
        .iter()
        .enumerate()
        .map(|(i, (field, unit))| {
            let error = errors
                .read()
                .get(*field)
                .and_then(|v| v.as_str())
                .map(|token| api::error_i18n::localize_field(field, token));
            let kind = if *unit == LimitUnit::Quantity {
                "text"
            } else {
                "number"
            };
            let value = texts.read()[i].clone();
            (i, *field, kind, value, default_texts[i].clone(), error)
        })
        .collect();
    let source = if info.overridden {
        t!("system-agents-overridden")
    } else {
        t!("system-agents-from-config")
    };
    rsx! {
        p { class: "text-xs text-gray-500", {source} }
        div { class: "grid gap-4 md:grid-cols-2",
            for (i, field, kind, value, default, error) in rows {
                div { key: "{field}",
                    label { r#for: "agent-limit-{field}", class: FIELD_LABEL, {limit_label(field)} }
                    input {
                        id: "agent-limit-{field}",
                        class: INPUT,
                        r#type: kind,
                        value: "{value}",
                        oninput: move |e| texts.with_mut(|t| t[i] = e.value()),
                    }
                    p { class: FIELD_HELP, {t!("system-agents-default", value : default)} }
                    if let Some(error) = error {
                        p { class: FIELD_ERROR, {error} }
                    }
                }
            }
        }
        div { class: "flex gap-2",
            button { class: BTN_PRIMARY_SM, disabled: busy(), onclick: save, {t!("common-save")} }
            button {
                class: BTN_SECONDARY_SM,
                disabled: busy(),
                onclick: move |_| texts.set(default_texts.clone()),
                {t!("system-agents-reset")}
            }
        }
    }
}

#[cfg(test)]
mod agent_limits_tests {
    use super::*;

    #[test]
    fn form_texts_read_back_into_limits() {
        let texts: Vec<String> = [
            "4Gi", "40Gi", "0.0625", "1024", "30", "8192", "14", "3650", "365", "7",
        ]
        .map(String::from)
        .to_vec();
        let l = limits_of(&texts).unwrap();
        assert_eq!(
            (l.artifact_max_bytes, l.artifact_run_max_bytes),
            (65536, 1 << 30)
        );
        assert_eq!((l.build_memory.as_str(), l.job_logs_days), ("4Gi", 7));
        let back = serde_json::to_value(&l).unwrap();
        assert_eq!(
            limit_text(&back, "artifact_max_bytes", LimitUnit::Mib),
            "0.0625"
        );
        let mut bad = texts.clone();
        bad[4] = "soon".into();
        assert_eq!(limits_of(&bad).unwrap_err(), "artifact_expires_days");
    }
}

#[cfg(test)]
mod tests {
    use super::parse_gb;

    #[test]
    fn table_quota_is_typed_in_gb() {
        assert_eq!(parse_gb(""), Ok(None));
        assert_eq!(parse_gb("20"), Ok(Some(20 * 1024 * 1024 * 1024)));
        assert_eq!(parse_gb("1,5"), Ok(Some(1_610_612_736)));
        assert!(parse_gb("0").is_err() && parse_gb("-3").is_err() && parse_gb("x").is_err());
    }
}
