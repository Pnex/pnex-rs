//! Builds of an environment (agent-environments.md D703, D720, D748): state, digest, scan
//! and its findings, activation on the version of the environment read, and the journal.

use dioxus::prelude::*;
use dioxus_i18n::t;

use super::short_date;
use crate::api;
use crate::components::ui::BTN_SECONDARY_SM;
use crate::state::access;
use crate::state::toasts;
use pnex_core::environments::{
    Environment, EnvironmentActivate, EnvironmentBuild, EnvironmentBuildState,
    EnvironmentScanState, ScanCounts, ScanMode, ScanPolicy, ScanThreshold,
};

fn build_state(b: &EnvironmentBuild) -> String {
    match b.state {
        EnvironmentBuildState::Building => t!("agents-build-building").to_string(),
        EnvironmentBuildState::Ready => t!("agents-build-ready").to_string(),
        EnvironmentBuildState::Failed => t!("agents-build-failed").to_string(),
    }
}

/// `host/repo@sha256:abcdef…` → `sha256:abcdef012345`.
fn short_digest(image: &str) -> String {
    let digest = image.rsplit_once('@').map(|(_, d)| d).unwrap_or(image);
    digest.chars().take(19).collect()
}

/// Whether the counts may be over the threshold. The counts do not say which findings are
/// fixable, so this is a hint; the findings dialog counts exactly (`ScanPolicy::breaches`).
fn may_breach(policy: &ScanPolicy, c: &ScanCounts) -> bool {
    let at_threshold = match policy.threshold {
        ScanThreshold::Critical => c.critical,
        ScanThreshold::High => c.critical + c.high,
        ScanThreshold::None => 0,
    };
    at_threshold > 0 && c.fixable > 0
}

#[component]
pub(super) fn BuildsPanel(
    env: Environment,
    reload: Signal<u32>,
    on_changed: Callback<()>,
) -> Element {
    let env_id = env.id.clone();
    let log_of = use_signal(|| Option::<String>::None);
    let findings_of = use_signal(|| Option::<EnvironmentBuild>::None);
    let list_id = env_id.clone();
    let builds = use_resource(move || {
        let id = list_id.clone();
        let _ = reload();
        async move { api::hosted_agents::builds(&id).await }
    });
    let policy = use_resource(|| async { api::hosted_agents::scan_policy().await });
    let logs_env = env_id.clone();
    let lines = use_resource(move || {
        let env = logs_env.clone();
        async move {
            match log_of() {
                Some(build) => api::hosted_agents::build_logs(&env, &build)
                    .await
                    .unwrap_or_default(),
                None => Vec::new(),
            }
        }
    });
    let rows = match &*builds.value().read() {
        Some(Ok(p)) => p.results.clone(),
        _ => Vec::new(),
    };
    let policy = policy
        .value()
        .read()
        .clone()
        .and_then(Result::ok)
        .unwrap_or_default();
    let shown = lines.value().read().clone().unwrap_or_default();
    let can_build = access::can("environments:build");
    rsx! {
        if let Some(build) = findings_of() {
            FindingsDialog {
                env_id: env_id.clone(),
                item: build,
                policy,
                findings_of,
            }
        }
        div { class: "space-y-3 rounded-xl border border-line bg-surface p-3",
            div { class: "flex items-center justify-between",
                h3 { class: "text-sm font-semibold text-ink", {t!("agents-builds")} }
                button {
                    class: BTN_SECONDARY_SM,
                    onclick: move |_| {
                        let mut reload = reload;
                        reload += 1;
                    },
                    {t!("agents-env-refresh")}
                }
            }
            if rows.is_empty() {
                p { class: "text-sm text-slate", {t!("agents-no-builds")} }
            }
            for b in rows {
                BuildRow {
                    key: "{b.id}",
                    item: b.clone(),
                    env: env.clone(),
                    policy,
                    can_build,
                    log_of,
                    findings_of,
                    on_changed,
                }
            }
            if log_of().is_some() {
                h3 { class: "text-sm font-semibold text-ink", {t!("agents-log")} }
                pre { class: "max-h-96 overflow-auto rounded bg-gray-50 p-3 text-xs leading-5 text-ink",
                    if shown.is_empty() {
                        {t!("agents-no-log")}
                    }
                    for (i, l) in shown.into_iter().enumerate() {
                        div { key: "{i}", "{l.line}" }
                    }
                }
            }
        }
    }
}

#[component]
fn BuildRow(
    item: EnvironmentBuild,
    env: Environment,
    policy: ScanPolicy,
    can_build: bool,
    log_of: Signal<Option<String>>,
    findings_of: Signal<Option<EnvironmentBuild>>,
    on_changed: Callback<()>,
) -> Element {
    let build = item;
    let mut log_of = log_of;
    let mut findings_of = findings_of;
    let active = env.active_build_id.as_deref() == Some(build.id.as_str());
    let activable = can_build
        && !active
        && build.state == EnvironmentBuildState::Ready
        && build.image.is_some();
    let scan = match build.scan_state {
        Some(EnvironmentScanState::Scanning) => Some(t!("agents-env-scan-scanning").to_string()),
        Some(EnvironmentScanState::Failed) => Some(t!("agents-env-scan-failed").to_string()),
        Some(EnvironmentScanState::Done) => None,
        None => None,
    };
    let counts = build.scan_counts.clone();
    let breach = counts.as_ref().is_some_and(|c| may_breach(&policy, c));
    let (env_id, build_id, version) = (env.id.clone(), build.id.clone(), env.version);
    let log_id = build.id.clone();
    let opened = build.clone();
    rsx! {
        div { class: "space-y-1 border-t border-line pt-2 text-sm",
            div { class: "flex flex-wrap items-center gap-2",
                span { class: "font-semibold text-ink", "v{build.number}" }
                span { class: "text-slate", {build_state(&build)} }
                if active {
                    span { class: "rounded-full border border-green-200 bg-green-50 px-2 text-xs text-green-800",
                        {t!("agents-env-active")}
                    }
                }
                span { class: "ml-auto text-xs text-slate", {short_date(&build.created_at)} }
            }
            if let Some(image) = build.image.clone() {
                div { class: "font-mono text-xs text-slate", title: "{image}",
                    {short_digest(&image)}
                }
            }
            if let Some(code) = build.outcome.clone() {
                div { class: "text-xs text-alarm",
                    {crate::api::error_i18n::localize_code(&code, None, &code)}
                }
            }
            if let Some(label) = scan {
                div { class: "text-xs text-slate", {label} }
            }
            if let Some(c) = counts {
                button {
                    class: "text-xs text-signal-700 hover:underline text-left",
                    onclick: move |_| findings_of.set(Some(opened.clone())),
                    {
                        t!(
                            "agents-env-scan-counts", critical : c.critical, high : c.high, fixable : c
                            .fixable
                        )
                    }
                }
            }
            if breach {
                div { class: "text-xs text-amber-700",
                    if policy.mode == ScanMode::Refuse {
                        {t!("agents-env-scan-over-refuse")}
                    } else {
                        {t!("agents-env-scan-over-signal")}
                    }
                }
            }
            div { class: "flex flex-wrap gap-2",
                button {
                    class: BTN_SECONDARY_SM,
                    onclick: move |_| log_of.set(Some(log_id.clone())),
                    {t!("agents-log")}
                }
                if activable {
                    button {
                        class: BTN_SECONDARY_SM,
                        onclick: move |_| {
                            let (env_id, build_id) = (env_id.clone(), build_id.clone());
                            spawn(async move {
                                let input = EnvironmentActivate {
                                    expected_version: version,
                                };
                                match api::hosted_agents::activate_build(&env_id, &build_id, &input).await {
                                    Ok(_) => {
                                        toasts::info(t!("agents-env-activated").to_string());
                                        on_changed.call(());
                                    }
                                    Err(e) => toasts::error(e),
                                }
                            });
                        },
                        {t!("agents-env-activate")}
                    }
                }
            }
        }
    }
}

/// Findings of the latest scan of a build, with the version that fixes each.
#[component]
fn FindingsDialog(
    env_id: String,
    item: EnvironmentBuild,
    policy: ScanPolicy,
    findings_of: Signal<Option<EnvironmentBuild>>,
) -> Element {
    let build = item;
    let mut findings_of = findings_of;
    let build_id = build.id.clone();
    let scan = use_resource(move || {
        let (env, build) = (env_id.clone(), build_id.clone());
        async move { api::hosted_agents::build_scan(&env, &build).await }
    });
    let report = match &*scan.value().read() {
        Some(Ok(s)) => s.report.clone(),
        _ => None,
    };
    let error = match &*scan.value().read() {
        Some(Err(e)) => Some(crate::api::error_i18n::localize(e)),
        _ => None,
    };
    let breaches = report.as_ref().map(|r| policy.breaches(r)).unwrap_or(0);
    let findings = report.map(|r| r.findings).unwrap_or_default();
    rsx! {
        crate::components::modal::Modal {
            title: t!("agents-env-findings-title", v : build.number).to_string(),
            max_width: "max-w-3xl".to_string(),
            on_close: move |_| findings_of.set(None),
            div { class: "space-y-3",
                if let Some(e) = error {
                    p { class: "text-sm text-alarm", "{e}" }
                }
                p { class: "text-sm text-ink",
                    {t!("agents-env-findings-breaches", count : breaches)}
                }
                if findings.is_empty() {
                    p { class: "text-sm text-slate", {t!("agents-env-findings-empty")} }
                } else {
                    table { class: "w-full text-xs",
                        thead {
                            tr { class: "text-left text-slate",
                                th { class: "py-1 pr-2", {t!("agents-env-finding-id")} }
                                th { class: "py-1 pr-2", {t!("agents-env-finding-severity")} }
                                th { class: "py-1 pr-2", {t!("agents-env-finding-package")} }
                                th { class: "py-1 pr-2", {t!("agents-env-finding-installed")} }
                                th { class: "py-1", {t!("agents-env-finding-fixed")} }
                            }
                        }
                        tbody {
                            for (i, f) in findings.into_iter().enumerate() {
                                tr { key: "{i}", class: "border-t border-line",
                                    td { class: "py-1 pr-2 font-mono", {f.id} }
                                    td { class: "py-1 pr-2", {f.severity} }
                                    td { class: "py-1 pr-2 font-mono", {f.package} }
                                    td { class: "py-1 pr-2 font-mono", {f.installed} }
                                    td { class: "py-1 font-mono", {f.fixed.unwrap_or_default()} }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}
