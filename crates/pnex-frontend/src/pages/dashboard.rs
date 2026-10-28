//! Dashboard — two data layers:
//! - **user** aggregates from `/api/v1/user-info` (Phase 3): device count
//!   across the user's orgs, orgs, tier;
//! - **org** summary from `/api/v1/dashboard/summary` (2026-08-19): device
//!   liveness (silence TTL), build stats, latest OpenObserve
//!   measurements — the "Devices online" and "Build success"
//!   cards and the two bottom sections. 15 s polling (the value stays
//!   displayed during the re-fetch), lists capped at ~10 server side
//!   ("shown / total" counter if truncated), degraded telemetry
//!   silent (notice, never a toast on repeated errors).

use dioxus::prelude::*;
use dioxus_i18n::t;

use crate::api;
use crate::components::badges::date_label;
use crate::components::crud::layout::ListLayout;
use crate::components::refresh_rate::{use_auto_refresh, RefreshRateControl};
use crate::components::ui::{State, StateMark, PANEL};
use crate::state::{org, session, toasts};

/// Summary refresh rate (liveness moves with the silence TTL
/// ~10 s; 15 s is enough for a dashboard).
const POLL_SECS: u64 = 15;

#[component]
pub fn Dashboard() -> Element {
    // All hooks run before the session early return: the hook order must
    // stay stable across renders.
    // Org-scoped summary: re-run when the org changes or on the reload counter
    // (button + polling). Errors swallowed → None (silent degradation).
    let reload = use_signal(|| 0u32);
    let summary = use_resource(move || async move {
        let _ = reload();
        // Reading the org signal: re-run on org change; no org
        // selected (yet) → nothing to load.
        org::current()?;
        api::dashboard::summary().await.ok()
    });
    // User-selectable auto-refresh (default 15 s) + "refresh now".
    let auto = use_auto_refresh(reload, POLL_SECS);

    let user = session::user();
    let Some(user) = user else {
        return rsx! {};
    };
    let org_id = org::current();
    let active_org = user.orgs.iter().find(|m| Some(m.id) == org_id);
    let tier_name = active_org
        .and_then(|m| m.subscription_tier.as_ref())
        .map(|tier| tier.name.clone())
        // No subscription tier on a self-hosted deployment: say so instead of a dash.
        .unwrap_or_else(|| {
            if user.deployment_mode == pnex_core::DeploymentMode::Saas {
                "—".into()
            } else {
                t!("dash-tier-self-hosted").to_string()
            }
        });
    // Tier quotas only apply in SaaS: hide them when self-hosted (O59).
    let tier = active_org
        .and_then(|m| m.subscription_tier.as_ref())
        .filter(|_| user.deployment_mode == pnex_core::DeploymentMode::Saas);
    // Resource: Option<Option<T>> (pending | degraded); None while
    // pending or on error (silent degradation).
    let s = match summary.read().as_ref() {
        Some(inner) => inner.clone(),
        None => None,
    };
    // Per-type counts of the active org (the summary), never the sum over
    // every org of the user: the quotas below are the org's.
    let mut by_type: Vec<(String, u64)> = s
        .as_ref()
        .map(|x| x.liveness.by_type.clone().into_iter().collect())
        .unwrap_or_default();
    by_type.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    let used_of = |name: &str| {
        by_type
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, c)| *c)
            .unwrap_or(0)
    };
    let (used_sensor, used_actuator, used_mixed) =
        (used_of("sensor"), used_of("actuator"), used_of("mixed"));
    let live_label = match &s {
        Some(x) => format!("{}/{}", x.liveness.live, x.liveness.total),
        None => "—".into(),
    };
    let total_label = match &s {
        Some(x) => x.liveness.total.to_string(),
        None => "—".into(),
    };
    let build_label = s
        .as_ref()
        .filter(|x| x.builds.total > 0)
        .map(|x| format!("{:.0}%", x.builds.success_rate * 100.0));
    let liveness_list = s.as_ref().map(|x| x.liveness.devices.as_slice());
    let liveness_total = s.as_ref().map(|x| x.liveness.total);
    // "shown / total" counter when the list is truncated (cap ~10
    // backend side).
    let liveness_more = match (liveness_list, liveness_total) {
        (Some(list), Some(total)) if total > list.len() as u64 => {
            Some(format!("{} / {}", list.len(), total))
        }
        _ => None,
    };
    let telemetry = s.as_ref().map(|x| &x.telemetry);

    rsx! {
        ListLayout {
            title: t!("nav-dashboard").to_string(),
            subtitle: t!("dash-subtitle").to_string(),
            can_write: false,
            actions: rsx! {
                RefreshRateControl {
                    auto,
                    on_refresh: move |_| {
                        spawn(async {
                            match crate::api::user::get_user_info().await {
                                Ok(fresh) => session::login(fresh),
                                Err(err) => toasts::error(err),
                            }
                        });
                    },
                }
            },
            // One readout band (instrument panel) rather than a card per number.
            div { class: "{PANEL} mb-6 grid grid-cols-2 divide-line md:grid-cols-5 md:divide-x",
                {readout(t!("dash-total-devices"), total_label, None)}
                {readout(t!("dash-live-sensors"), live_label, None)}
                {
                    readout(
                        t!("dash-build-success"),
                        build_label.unwrap_or_else(|| "—".into()),
                        s.as_ref().filter(|x| x.builds.total == 0).map(|_| t!("dash-no-builds")),
                    )
                }
                {readout(t!("dash-orgs"), user.orgs.len().to_string(), None)}
                {readout(t!("dash-tier"), tier_name, None)}
            }

            div { class: "mb-6 grid grid-cols-1 gap-6 lg:grid-cols-2",
                Section { title: t!("dash-liveness"),
                    match liveness_list.filter(|list| !list.is_empty()) {
                        None => rsx! {
                            p { class: "py-6 text-center text-sm text-slate", {t!("dash-no-devices")} }
                        },
                        Some(list) => rsx! {
                            ul { class: "divide-y divide-line",
                                for device in list {
                                    li {
                                        key: "{device.id}",
                                        class: "flex items-center justify-between gap-3 py-2.5",
                                        div { class: "min-w-0",
                                            p { class: "truncate font-mono text-sm font-medium text-ink",
                                                {device.device_id.clone()}
                                            }
                                            p { class: "truncate text-xs text-slate",
                                                "{device.predefined_device_name} · {device.device_type}"
                                            }
                                        }
                                        StateMark {
                                            state: if device.live { State::Ok } else if device.last_seen.is_some() { State::Idle } else { State::Never },
                                            label: device
                                                .last_seen
                                                .as_deref()
                                                .map(date_label)
                                                .unwrap_or_else(|| t!("dash-never").to_string()),
                                        }
                                    }
                                }
                            }
                            if let Some(counter) = &liveness_more {
                                p { class: "mt-2 text-right text-xs text-slate", {counter.clone()} }
                            }
                        },
                    }
                }

                Section { title: t!("dash-last-measurements"),
                    match telemetry {
                        Some(t) if t.available && !t.latest.is_empty() => rsx! {
                            div { class: "overflow-x-auto",
                                table { class: "w-full text-sm",
                                    thead {
                                        tr { class: "border-b border-line text-left text-xs text-slate",
                                            th { class: "py-2 pr-4 font-medium", {t!("dash-col-device")} }
                                            th { class: "py-2 pr-4 font-medium", {t!("dash-col-metric")} }
                                            th { class: "py-2 pr-4 text-right font-medium", {t!("dash-col-value")} }
                                            th { class: "hidden py-2 font-medium sm:table-cell", {t!("dash-col-time")} }
                                        }
                                    }
                                    tbody {
                                        for m in &t.latest {
                                            tr {
                                                key: "{m.metric}-{m.device_id}-{m.timestamp.as_deref().unwrap_or_default()}",
                                                class: "border-b border-line last:border-0",
                                                td { class: "break-all py-2 pr-4 font-mono text-ink", {m.device_id.clone()} }
                                                td { class: "py-2 pr-4 text-slate", {m.metric.clone()} }
                                                td { class: "py-2 pr-4 text-right font-semibold tabular-nums text-ink",
                                                    "{m.value}"
                                                }
                                                td { class: "hidden py-2 text-xs text-slate sm:table-cell",
                                                    {m.timestamp.as_deref().map(date_label).unwrap_or_else(|| "—".into())}
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        },
                        Some(t) if t.available => rsx! {
                            p { class: "py-6 text-center text-sm text-slate", {t!("dash-no-measurements")} }
                        },
                        _ => rsx! {
                            p { class: "rounded-md border border-line bg-gray-50 p-4 text-sm text-slate",
                                {t!("dash-telemetry-unavailable")}
                            }
                        },
                    }
                }
            }

            div { class: "grid grid-cols-1 gap-6 lg:grid-cols-2",
                Section { title: t!("dash-active-org"),
                    if let Some(membership) = active_org {
                        div { class: "space-y-4",
                            div { class: "flex items-center justify-between",
                                p { class: "font-medium text-ink", {membership.name.clone()} }
                                {
                                    let (badge, label) = crate::pages::orgs::role_badge(membership.role);
                                    rsx! {
                                        span { class: "inline-flex items-center rounded-full px-2.5 py-0.5 text-xs font-medium {badge}",
                                            {label}
                                        }
                                    }
                                }
                            }
                            if let Some(tier) = tier {
                                div { class: "space-y-2",
                                    p { class: "text-sm font-medium text-ink", {t!("dash-quotas")} }
                                    {
                                        quota_row(
                                            t!("dash-quota-sensor"),
                                            used_sensor,
                                            tier.max_sensor_devices.max(0) as u64,
                                        )
                                    }
                                    {
                                        quota_row(
                                            t!("dash-quota-actuator"),
                                            used_actuator,
                                            tier.max_actuator_devices.max(0) as u64,
                                        )
                                    }
                                    {
                                        quota_row(
                                            t!("dash-quota-mixed"),
                                            used_mixed,
                                            tier.max_mixed_devices.max(0) as u64,
                                        )
                                    }
                                }
                            }
                        }
                    } else {
                        p { class: "py-6 text-center text-sm text-slate", {t!("orgs-empty")} }
                    }
                }

                Section { title: t!("dash-by-type"),
                    if by_type.is_empty() {
                        p { class: "py-6 text-center text-sm text-slate", {t!("dash-no-devices")} }
                    } else {
                        ul { class: "divide-y divide-line",
                            for (type_name, count) in by_type.clone() {
                                li {
                                    key: "{type_name}",
                                    class: "flex items-center justify-between py-2.5 text-sm",
                                    span { class: "text-ink", {type_name} }
                                    span { class: "font-semibold tabular-nums text-ink",
                                        "{count}"
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

/// One value of the readout band: label above, value in the display size.
fn readout(label: String, value: String, hint: Option<String>) -> Element {
    rsx! {
        div { class: "border-line px-5 py-4 [&:nth-child(odd)]:border-r md:[&:nth-child(odd)]:border-r-0",
            p { class: "text-xs text-slate", {label} }
            p { class: "mt-1 truncate text-2xl font-semibold tabular-nums text-ink",
                {value}
            }
            if let Some(hint) = hint {
                p { class: "mt-0.5 text-xs text-slate", {hint} }
            }
        }
    }
}

/// Titled work surface of the home page.
#[component]
fn Section(title: String, children: Element) -> Element {
    rsx! {
        section { class: PANEL,
            h2 { class: "border-b border-line px-5 py-3 text-base font-semibold text-ink",
                {title}
            }
            div { class: "px-5 py-3", {children} }
        }
    }
}

fn quota_row(label: String, used: u64, max: u64) -> Element {
    rsx! {
        div { class: "flex items-center justify-between text-sm",
            span { class: "text-slate", {label} }
            span { class: "font-medium tabular-nums text-ink", "{used} / {max}" }
        }
    }
}
