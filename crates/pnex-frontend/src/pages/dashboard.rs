//! Tableau de bord — deux strates de données :
//! - agrégats **user** de `/api/v1/user-info` (Phase 3) : comptage devices
//!   sur les orgs du user, orgs, tier ;
//! - summary **org** de `/api/v1/dashboard/summary` (2026-08-19) : liveness
//!   des devices (TTL de silence), stats builds, dernières mesures
//!   OpenObserve — les cartes « Appareils en ligne » et « Réussite des
//!   builds » et les deux sections en bas. Polling 15 s, télémétrie
//!   dégradée silencieuse (encart, jamais de toast en erreur répétée).

use std::time::Duration;

use dioxus::prelude::*;
use dioxus_i18n::t;

use crate::api;
use crate::components::badges::date_label;
use crate::components::icons;
use crate::state::{org, session, toasts};
use crate::util::sleep;

/// Cadence de rafraîchissement du summary (liveness bouge au TTL de
/// silence ~10 s ; 15 s suffit pour un dashboard).
const POLL_SECS: u64 = 15;

#[component]
pub fn Dashboard() -> Element {
    let user = session::user();

    let Some(user) = user else {
        return rsx! {};
    };
    let org_id = org::current();
    let active_org = user.orgs.iter().find(|m| Some(m.id) == org_id);
    let tier_name = active_org
        .and_then(|m| m.subscription_tier.as_ref())
        .map(|tier| tier.name.clone())
        .unwrap_or_else(|| "—".into());
    let tier = active_org.and_then(|m| m.subscription_tier.as_ref());
    let mut by_type: Vec<(String, u64)> = user.device_count.by_type.clone().into_iter().collect();
    by_type.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    // Usage réel par type (comptage devices agrégé sur les orgs de l'user).
    let used_of = |name: &str| user.device_count.by_type.get(name).copied().unwrap_or(0);
    let (used_sensor, used_actuator, used_mixed) =
        (used_of("sensor"), used_of("actuator"), used_of("mixed"));

    // Summary org-scope : re-run quand l'org change ou au compteur reload
    // (bouton + polling). Erreurs avalées → None (dégradé silencieux).
    let mut reload = use_signal(|| 0u32);
    let summary = use_resource(move || async move {
        let _ = reload();
        // Lecture du signal org : re-run au changement d'org ; pas d'org
        // (encore) sélectionnée → rien à charger.
        org::current()?;
        api::dashboard::summary().await.ok()
    });
    // Ressource : Option<Option<T>> (pending | dégradé) ; None tant que
    // pending ou en erreur (dégradé silencieux).
    let s = match summary.read().as_ref() {
        Some(inner) => inner.clone(),
        None => None,
    };
    // Valeurs dérivées précalculées : les expressions des enfants rsx sont
    // évaluées paresseusement (capturées par référence), on ne creuse pas
    // l'Option dedans.
    let live_label = match &s {
        Some(x) => format!("{}/{}", x.liveness.live, x.liveness.total),
        None => "—".into(),
    };
    let build_label = s
        .as_ref()
        .filter(|x| x.builds.total > 0)
        .map(|x| format!("{:.0}%", x.builds.success_rate * 100.0));
    let liveness_list = s.as_ref().map(|x| x.liveness.devices.as_slice());
    let telemetry = s.as_ref().map(|x| &x.telemetry);

    // Polling auto-entretenu tant que la page est montée.
    let mut polling = use_signal(|| false);
    if !polling() {
        polling.set(true);
        spawn(async move {
            sleep(Duration::from_secs(POLL_SECS)).await;
            polling.set(false);
            reload.with_mut(|r| *r += 1);
        });
    }

    rsx! {
        div { class: "p-6",
            div { class: "mb-8 flex items-center justify-between",
                div {
                    h1 { class: "text-3xl font-bold text-gray-900", {t!("nav-dashboard")} }
                    p { class: "text-gray-600 mt-2", {t!("dash-subtitle")} }
                }
                button {
                    class: "inline-flex items-center px-3 py-2 text-sm text-gray-600 border border-gray-300 rounded-lg hover:bg-gray-50 transition-colors",
                    onclick: move |_| {
                        reload.with_mut(|r| *r += 1);
                        spawn(async {
                            match crate::api::user::get_user_info().await {
                                Ok(fresh) => session::login(fresh),
                                Err(err) => toasts::error(err.message),
                            }
                        });
                    },
                    icons::RefreshCw { class: "h-4 w-4 mr-2" }
                    {t!("common-retry")}
                }
            }

            // Cartes stats user (agrégats sur les orgs du user)
            div { class: "grid grid-cols-1 md:grid-cols-2 lg:grid-cols-4 gap-6 mb-8",
                {stat_card("border-blue-500", "bg-blue-100", t!("dash-total-devices"),
                    user.device_count.total.to_string(),
                    rsx! { icons::Cpu { class: "h-6 w-6 text-blue-600" } })}
                {stat_card("border-green-500", "bg-green-100", t!("dash-active-devices"),
                    user.device_count.active.to_string(),
                    rsx! { icons::CheckCircle { class: "h-6 w-6 text-green-600" } })}
                {stat_card("border-purple-500", "bg-purple-100", t!("dash-orgs"),
                    user.orgs.len().to_string(),
                    rsx! { icons::Building { class: "h-6 w-6 text-purple-600" } })}
                {stat_card("border-orange-500", "bg-orange-100", t!("dash-tier"),
                    tier_name,
                    rsx! { icons::Zap { class: "h-6 w-6 text-orange-600" } })}
            }

            // Cartes stats org (summary) — « en ligne » = frais au TTL de
            // silence, ≠ « actifs » ci-dessus (booléen reaper).
            div { class: "grid grid-cols-1 md:grid-cols-2 gap-6 mb-8",
                div { class: "bg-white p-6 rounded-lg shadow-sm border-l-4 border-teal-500",
                    div { class: "flex items-center justify-between",
                        div {
                            p { class: "text-sm font-medium text-gray-600", {t!("dash-live-sensors")} }
                            p { class: "text-3xl font-bold text-gray-900", {live_label} }
                        }
                        div { class: "p-3 rounded-full bg-teal-100",
                            icons::Wifi { class: "h-6 w-6 text-teal-600" }
                        }
                    }
                }
                div { class: "bg-white p-6 rounded-lg shadow-sm border-l-4 border-indigo-500",
                    div { class: "flex items-center justify-between",
                        div {
                            p { class: "text-sm font-medium text-gray-600", {t!("dash-build-success")} }
                            if let Some(pct) = &build_label {
                                p { class: "text-3xl font-bold text-gray-900", {pct.clone()} }
                            } else {
                                p { class: "text-3xl font-bold text-gray-900", "—" }
                                p { class: "text-xs text-gray-400 mt-1", {t!("dash-no-builds")} }
                            }
                        }
                        div { class: "p-3 rounded-full bg-indigo-100",
                            icons::Package { class: "h-6 w-6 text-indigo-600" }
                        }
                    }
                }
            }

            // Liveness + dernières mesures (summary)
            div { class: "grid grid-cols-1 lg:grid-cols-2 gap-8 mb-8",
                div { class: "bg-white rounded-lg shadow-sm",
                    div { class: "p-6 border-b border-gray-200",
                        h2 { class: "text-lg font-semibold text-gray-900", {t!("dash-liveness")} }
                    }
                    div { class: "p-6",
                        match liveness_list.filter(|list| !list.is_empty()) {
                            None => rsx! {
                                p { class: "text-gray-500 text-center py-8", {t!("dash-no-devices")} }
                            },
                            Some(list) => rsx! {
                                div { class: "space-y-2",
                                    for device in list {
                                        div { key: "{device.id}", class: "flex items-center justify-between p-3 bg-gray-50 rounded-lg",
                                            div { class: "flex items-center space-x-3",
                                                span { class: if device.live { "h-2.5 w-2.5 rounded-full bg-green-500" } else { "h-2.5 w-2.5 rounded-full bg-gray-300" } }
                                                div {
                                                    p { class: "text-sm font-medium text-gray-900", {device.device_id.clone()} }
                                                    p { class: "text-xs text-gray-500",
                                                        "{device.predefined_device_name} · {device.device_type}"
                                                    }
                                                }
                                            }
                                            span { class: "text-xs text-gray-500",
                                                {device.last_seen.as_deref().map(date_label).unwrap_or_else(|| t!("dash-never"))}
                                            }
                                        }
                                    }
                                }
                            },
                        }
                    }
                }

                div { class: "bg-white rounded-lg shadow-sm",
                    div { class: "p-6 border-b border-gray-200",
                        h2 { class: "text-lg font-semibold text-gray-900", {t!("dash-last-measurements")} }
                    }
                    div { class: "p-6",
                        match telemetry {
                            Some(t) if t.available && !t.latest.is_empty() => rsx! {
                                table { class: "w-full text-sm",
                                    thead {
                                        tr { class: "text-left text-xs text-gray-500 uppercase border-b border-gray-200",
                                            th { class: "py-2 pr-4", {t!("dash-col-device")} }
                                            th { class: "py-2 pr-4", {t!("dash-col-metric")} }
                                            th { class: "py-2 pr-4 text-right", {t!("dash-col-value")} }
                                            th { class: "py-2", {t!("dash-col-time")} }
                                        }
                                    }
                                    tbody {
                                        for m in &t.latest {
                                            tr { key: "{m.metric}-{m.device_id}-{m.timestamp.as_deref().unwrap_or_default()}",
                                                class: "border-b border-gray-100",
                                                td { class: "py-2 pr-4 font-medium text-gray-900", {m.device_id.clone()} }
                                                td { class: "py-2 pr-4 text-gray-700", {m.metric.clone()} }
                                                td { class: "py-2 pr-4 text-right font-bold text-gray-900", "{m.value}" }
                                                td { class: "py-2 text-xs text-gray-500",
                                                    {m.timestamp.as_deref().map(date_label).unwrap_or_else(|| "—".into())}
                                                }
                                            }
                                        }
                                    }
                                }
                            },
                            Some(t) if t.available => rsx! {
                                p { class: "text-gray-500 text-center py-8", {t!("dash-no-measurements")} }
                            },
                            _ => rsx! {
                                p { class: "text-sm text-gray-500 bg-gray-50 border border-gray-200 rounded-lg p-4",
                                    {t!("dash-telemetry-unavailable")}
                                }
                            },
                        }
                    }
                }
            }

            div { class: "grid grid-cols-1 lg:grid-cols-2 gap-8",
                // Organisation active
                div { class: "bg-white rounded-lg shadow-sm",
                    div { class: "p-6 border-b border-gray-200",
                        h2 { class: "text-lg font-semibold text-gray-900", {t!("dash-active-org")} }
                    }
                    div { class: "p-6 space-y-4",
                        if let Some(membership) = active_org {
                            div { class: "flex items-center justify-between p-4 bg-gray-50 rounded-lg",
                                p { class: "font-medium text-gray-900", {membership.name.clone()} }
                                {let (badge, label) = crate::pages::orgs::role_badge(&membership.role);
                                rsx! {
                                    span { class: "inline-flex items-center px-2.5 py-0.5 rounded-full text-xs font-medium {badge}", {label} }
                                }}
                            }
                            if let Some(tier) = tier {
                                // Quotas : usage réel / plafond du tier de
                                // l'org active.
                                div { class: "p-4 bg-gray-50 rounded-lg space-y-2",
                                    p { class: "text-sm font-medium text-gray-700", {t!("dash-quotas")} }
                                    {quota_row(t!("dash-quota-sensor"), used_sensor, tier.max_sensor_devices.max(0) as u64)}
                                    {quota_row(t!("dash-quota-actuator"), used_actuator, tier.max_actuator_devices.max(0) as u64)}
                                    {quota_row(t!("dash-quota-mixed"), used_mixed, tier.max_mixed_devices.max(0) as u64)}
                                }
                            }
                        } else {
                            p { class: "text-gray-500 text-center py-8", {t!("orgs-empty")} }
                        }
                    }
                }

                // Devices par type
                div { class: "bg-white rounded-lg shadow-sm",
                    div { class: "p-6 border-b border-gray-200",
                        h2 { class: "text-lg font-semibold text-gray-900", {t!("dash-by-type")} }
                    }
                    div { class: "p-6",
                        if by_type.is_empty() {
                            p { class: "text-gray-500 text-center py-8", {t!("dash-no-devices")} }
                        } else {
                            div { class: "space-y-2",
                                for (type_name, count) in by_type {
                                    div { key: "{type_name}", class: "flex items-center justify-between p-3 bg-gray-50 rounded-lg",
                                        span { class: "text-sm font-medium text-gray-900", {type_name} }
                                        span { class: "text-sm font-bold text-gray-700", "{count}" }
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

fn stat_card(
    border: &'static str,
    icon_bg: &'static str,
    label: String,
    value: String,
    icon: Element,
) -> Element {
    rsx! {
        div { class: "bg-white p-6 rounded-lg shadow-sm border-l-4 {border}",
            div { class: "flex items-center justify-between",
                div {
                    p { class: "text-sm font-medium text-gray-600", {label} }
                    p { class: "text-3xl font-bold text-gray-900", {value} }
                }
                div { class: "p-3 rounded-full {icon_bg}", {icon} }
            }
        }
    }
}

fn quota_row(label: String, used: u64, max: u64) -> Element {
    rsx! {
        div { class: "flex items-center justify-between text-sm",
            span { class: "text-gray-600", {label} }
            span { class: "font-medium text-gray-900", "{used} / {max}" }
        }
    }
}
