//! Tableau de bord — porté du `Dashboard.tsx` React, sur les données réelles
//! de `/api/v1/user-info` (Phase 3) : comptage devices agrégé, orgs, tier.
//! Les cartes « Build Success Rate » et « Live Sensors » de l'original
//! reviennent avec les phases 6 (builds) et 5 (télémétrie).

use dioxus::prelude::*;
use dioxus_i18n::t;

use crate::components::icons;
use crate::state::{org, session, toasts};

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
    let mut by_type: Vec<(String, u64)> = user
        .device_count
        .by_type
        .clone()
        .into_iter()
        .collect();
    by_type.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));

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

            // Cartes stats
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
                                // Capacités du tier (les compteurs par catégorie
                                // arrivent avec les devices en Phase 4).
                                div { class: "p-4 bg-gray-50 rounded-lg space-y-2",
                                    p { class: "text-sm font-medium text-gray-700", {t!("dash-quotas")} }
                                    {quota_row(t!("dash-quota-sensor"), tier.max_sensor_devices.max(0) as u64)}
                                    {quota_row(t!("dash-quota-actuator"), tier.max_actuator_devices.max(0) as u64)}
                                    {quota_row(t!("dash-quota-mixed"), tier.max_mixed_devices.max(0) as u64)}
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

fn quota_row(label: String, max: u64) -> Element {
    rsx! {
        div { class: "flex items-center justify-between text-sm",
            span { class: "text-gray-600", {label} }
            span { class: "font-medium text-gray-900", "{max}" }
        }
    }
}
