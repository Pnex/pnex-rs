//! Home — the overview of the active org across every surface, not only
//! devices:
//! - a readout band (open cases, pages, library, deployed flows, devices
//!   online), each value a link to its page;
//! - "pick up where you left off": recent pages, open cases, latest flows;
//! - the workspaces (one tile per navigation group);
//! - the device block (liveness, latest measurements, tier quotas) only when
//!   the org has devices, from `/api/v1/dashboard/summary`.
//!
//! Every source degrades silently: a failed or forbidden list shows "—"
//! (never a toast on repeated polling errors). 15 s polling by default.

use dioxus::prelude::*;
use dioxus_i18n::t;
use pnex_core::flow::FlowSummary;
use pnex_core::ontology::api::{FilterOp, OntologyQuery, PropertyFilter, QueryResult};
use pnex_core::ontology::{CASE_STATUS, KIND_CASE};
use pnex_core::pages::DocSummary;
use pnex_core::pagination::Paginated;

use crate::api;
use crate::app::Route;
use crate::components::badges::date_label;
use crate::components::crud::layout::ListLayout;
use crate::components::refresh_rate::{use_auto_refresh, RefreshRateControl};
use crate::components::ui::{State, StateMark, PANEL};
use crate::state::{org, session, toasts};

/// Summary refresh rate (liveness moves with the silence TTL
/// ~10 s; 15 s is enough for a home page).
const POLL_SECS: u64 = 15;

/// Rows per "recent" list.
const RECENT: u32 = 5;

/// Cross-surface overview of the active org; each field is `None` when its
/// source failed or is forbidden to the caller.
#[derive(Default)]
struct Work {
    cases: Option<QueryResult>,
    docs: Option<Paginated<DocSummary>>,
    flows: Option<Paginated<FlowSummary>>,
    deployed: Option<i64>,
    media: Option<i64>,
}

async fn load_work() -> Work {
    let cases = OntologyQuery {
        type_key: Some(KIND_CASE.to_string()),
        filters: vec![PropertyFilter {
            key: CASE_STATUS.to_string(),
            op: FilterOp::Eq,
            value: serde_json::Value::String("open".into()),
        }],
        limit: Some(RECENT as u64),
        ..Default::default()
    };
    let flows = api::flows::FlowFilters {
        limit: Some(RECENT as i64),
        ..Default::default()
    };
    let deployed = api::flows::FlowFilters {
        status: Some("deployed".into()),
        limit: Some(1),
        ..Default::default()
    };
    let media = api::media::MediaFilters {
        limit: Some(1),
        ..Default::default()
    };
    let (cases, docs, flows, deployed, media) = futures::join!(
        api::ontology::query(&cases),
        api::docs::recent(RECENT),
        api::flows::list(&flows),
        api::flows::list(&deployed),
        api::media::list(&media),
    );
    Work {
        cases: cases.ok(),
        docs: docs.ok(),
        flows: flows.ok(),
        deployed: deployed.ok().map(|p| p.count),
        media: media.ok().map(|p| p.count),
    }
}

#[component]
pub fn Dashboard() -> Element {
    // All hooks run before the session early return: the hook order must
    // stay stable across renders.
    // Org-scoped sources: re-run when the org changes or on the reload
    // counter (button + polling). Errors swallowed → None (silent degradation).
    let reload = use_signal(|| 0u32);
    let summary = use_resource(move || async move {
        let _ = reload();
        // Reading the org signal: re-run on org change; no org
        // selected (yet) → nothing to load.
        org::current()?;
        api::dashboard::summary().await.ok()
    });
    let work = use_resource(move || async move {
        let _ = reload();
        org::current()?;
        Some(load_work().await)
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
    let name = user
        .full_name
        .clone()
        .filter(|n| !n.trim().is_empty())
        .unwrap_or_else(|| user.username.clone());
    let subtitle = match active_org {
        Some(m) => format!("{} · {tier_name}", m.name),
        None => tier_name,
    };

    // Resource: Option<Option<T>> (pending | degraded).
    let s = summary.read().as_ref().cloned().flatten();
    let work_guard = work.read();
    let w = work_guard.as_ref().and_then(|w| w.as_ref());
    let count = |v: Option<i64>| v.map(|n| n.to_string()).unwrap_or_else(|| "—".into());

    let devices_label = match &s {
        Some(x) => format!("{}/{}", x.liveness.live, x.liveness.total),
        None => "—".into(),
    };
    let flows_label = match (w.and_then(|w| w.deployed), w.and_then(|w| w.flows.as_ref())) {
        (Some(d), Some(f)) => format!("{d}/{}", f.count),
        _ => "—".into(),
    };
    let has_devices = s.as_ref().is_some_and(|x| x.liveness.total > 0);
    let cases_label = count(w.and_then(|w| w.cases.as_ref().map(|c| c.count)));
    let docs_label = count(w.and_then(|w| w.docs.as_ref().map(|d| d.count)));
    let media_label = count(w.and_then(|w| w.media));
    let doc_rows: Option<Vec<Row>> = w.and_then(|w| w.docs.as_ref()).map(|p| {
        p.results
            .iter()
            .map(|d| Row {
                to: Route::Docs {
                    id: d.id.to_string(),
                },
                title: d.title.clone(),
                meta: date_label(&d.updated_at),
                badge: None,
            })
            .collect()
    });
    let case_rows: Option<Vec<Row>> = w.and_then(|w| w.cases.as_ref()).map(|q| {
        q.rows
            .iter()
            .map(|r| Row {
                to: Route::CaseWorkspace {
                    id: r.object.id.clone(),
                },
                title: r.object.title.clone(),
                meta: date_label(&r.object.updated_at),
                badge: None,
            })
            .collect()
    });
    let flow_rows: Option<Vec<Row>> = w.and_then(|w| w.flows.as_ref()).map(|p| {
        p.results
            .iter()
            .map(|f| Row {
                to: Route::Flows {
                    id: f.id.to_string(),
                },
                title: f.name.clone(),
                meta: date_label(&f.updated_at),
                badge: Some(crate::pages::flows::status_badge(f.status.as_str())),
            })
            .collect()
    });

    rsx! {
        ListLayout {
            title: t!("home-greeting", name : name),
            subtitle,
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
                {readout(t!("home-open-cases"), cases_label, Route::Cases {})}
                {readout(t!("nav-docs"), docs_label, Route::Docs { id: String::new() })}
                {readout(t!("nav-library"), media_label, Route::Media {})}
                {
                    readout(
                        t!("home-flows-deployed"),
                        flows_label,
                        Route::Flows { id: String::new() },
                    )
                }
                {readout(t!("dash-live-sensors"), devices_label, Route::Devices {})}
            }

            div { class: "mb-6 grid grid-cols-1 gap-6 lg:grid-cols-3",
                Section {
                    title: t!("home-recent-pages"),
                    to: Route::Docs { id: String::new() },
                    RecentList { rows: doc_rows }
                }
                Section { title: t!("home-open-cases"), to: Route::Cases {},
                    RecentList { rows: case_rows }
                }
                Section {
                    title: t!("home-recent-flows"),
                    to: Route::Flows { id: String::new() },
                    RecentList { rows: flow_rows }
                }
            }

            h2 { class: "mb-3 text-base font-semibold text-ink", {t!("home-spaces")} }
            div { class: "mb-6 grid grid-cols-1 gap-3 sm:grid-cols-2 lg:grid-cols-5",
                Space {
                    to: Route::Dashboards {
                        id: String::new(),
                        mode: String::new(),
                    },
                    title: t!("nav-group-monitor"),
                    desc: t!("home-space-monitor"),
                    crate::components::icons::Gauge { class: "h-5 w-5" }
                }
                Space {
                    to: Route::Docs { id: String::new() },
                    title: t!("nav-group-knowledge"),
                    desc: t!("home-space-knowledge"),
                    crate::components::icons::BookOpen { class: "h-5 w-5" }
                }
                Space {
                    to: Route::Media {},
                    title: t!("nav-group-media"),
                    desc: t!("home-space-media"),
                    crate::components::icons::Image { class: "h-5 w-5" }
                }
                Space {
                    to: Route::Devices {},
                    title: t!("nav-group-devices"),
                    desc: t!("home-space-sources"),
                    crate::components::icons::Router { class: "h-5 w-5" }
                }
                Space {
                    to: Route::Flows { id: String::new() },
                    title: t!("nav-automation"),
                    desc: t!("home-space-automation"),
                    crate::components::icons::Workflow { class: "h-5 w-5" }
                }
            }

            // Device block: only for an org that has devices.
            if has_devices {
                if let Some(x) = &s {
                    div { class: "grid grid-cols-1 gap-6 lg:grid-cols-2",
                        Section { title: t!("dash-liveness"), to: Route::Devices {},
                            ul { class: "divide-y divide-line",
                                for device in &x.liveness.devices {
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
                            // "shown / total" when the list is capped server side (~10).
                            if x.liveness.total > x.liveness.devices.len() as u64 {
                                p { class: "mt-2 text-right text-xs text-slate",
                                    "{x.liveness.devices.len()} / {x.liveness.total}"
                                }
                            }
                            if let Some(tier) = tier {
                                div { class: "mt-3 space-y-2 border-t border-line pt-3",
                                    p { class: "text-sm font-medium text-ink", {t!("dash-quotas")} }
                                    {
                                        quota_row(
                                            t!("dash-quota-sensor"),
                                            by_type(x, "sensor"),
                                            tier.max_sensor_devices.max(0) as u64,
                                        )
                                    }
                                    {
                                        quota_row(
                                            t!("dash-quota-actuator"),
                                            by_type(x, "actuator"),
                                            tier.max_actuator_devices.max(0) as u64,
                                        )
                                    }
                                    {
                                        quota_row(
                                            t!("dash-quota-mixed"),
                                            by_type(x, "mixed"),
                                            tier.max_mixed_devices.max(0) as u64,
                                        )
                                    }
                                }
                            }
                        }
                        Section {
                            title: t!("dash-last-measurements"),
                            to: Route::Visualisation {},
                            {measurements(&x.telemetry)}
                        }
                    }
                }
            }
        }
    }
}

/// Device count of one type in the summary.
fn by_type(s: &pnex_core::DashboardSummary, name: &str) -> u64 {
    s.liveness.by_type.get(name).copied().unwrap_or(0)
}

/// One value of the readout band: label above, value in the display size;
/// the whole cell links to the page behind the number.
fn readout(label: String, value: String, to: Route) -> Element {
    rsx! {
        Link {
            to,
            class: "block border-line px-5 py-4 hover:bg-gray-50 [&:nth-child(odd)]:border-r md:[&:nth-child(odd)]:border-r-0",
            p { class: "text-xs text-slate", {label} }
            p { class: "mt-1 truncate text-2xl font-semibold tabular-nums text-ink",
                {value}
            }
        }
    }
}

/// Titled work surface of the home page, with a "see all" link.
#[component]
fn Section(title: String, to: Route, children: Element) -> Element {
    rsx! {
        section { class: PANEL,
            div { class: "flex items-center justify-between border-b border-line px-5 py-3",
                h2 { class: "text-base font-semibold text-ink", {title} }
                Link {
                    to,
                    class: "text-xs font-medium text-signal hover:underline",
                    {t!("home-see-all")}
                }
            }
            div { class: "px-5 py-3", {children} }
        }
    }
}

/// One line of a "recent" list.
#[derive(Clone, PartialEq)]
struct Row {
    to: Route,
    title: String,
    meta: String,
    badge: Option<(&'static str, String)>,
}

/// Recent items; `None` = source unavailable, shown as empty too.
#[component]
fn RecentList(rows: Option<Vec<Row>>) -> Element {
    let rows = rows.unwrap_or_default();
    if rows.is_empty() {
        return rsx! {
            p { class: "py-6 text-center text-sm text-slate", {t!("home-empty")} }
        };
    }
    rsx! {
        ul { class: "divide-y divide-line",
            for row in rows {
                li { class: "py-2",
                    Link {
                        to: row.to,
                        class: "flex items-center justify-between gap-3 hover:text-signal",
                        span { class: "min-w-0 truncate text-sm font-medium text-ink",
                            {row.title}
                        }
                        span { class: "flex shrink-0 items-center gap-2",
                            if let Some((badge, label)) = row.badge {
                                span { class: "rounded-full px-2 py-0.5 text-xs font-medium {badge}",
                                    {label}
                                }
                            }
                            span { class: "text-xs text-slate", {row.meta} }
                        }
                    }
                }
            }
        }
    }
}

/// Workspace tile: icon, group name, what lives there.
#[component]
fn Space(to: Route, title: String, desc: String, children: Element) -> Element {
    rsx! {
        Link {
            to,
            class: "{PANEL} block p-4 transition-colors hover:border-signal",
            div { class: "flex items-center gap-2 text-ink",
                {children}
                span { class: "font-semibold", {title} }
            }
            p { class: "mt-1.5 text-xs text-slate", {desc} }
        }
    }
}

/// Latest value per device and metric (degraded notice when the store is
/// unreachable).
fn measurements(telemetry: &pnex_core::dashboard::TelemetrySummary) -> Element {
    if !telemetry.available {
        return rsx! {
            p { class: "rounded-md border border-line bg-gray-50 p-4 text-sm text-slate",
                {t!("dash-telemetry-unavailable")}
            }
        };
    }
    if telemetry.latest.is_empty() {
        return rsx! {
            p { class: "py-6 text-center text-sm text-slate", {t!("dash-no-measurements")} }
        };
    }
    rsx! {
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
                    for m in &telemetry.latest {
                        tr {
                            key: "{m.metric}-{m.device_id}-{m.timestamp.as_deref().unwrap_or_default()}",
                            class: "border-b border-line last:border-0",
                            td { class: "break-all py-2 pr-4 font-mono text-ink",
                                {m.device_id.clone()}
                            }
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
