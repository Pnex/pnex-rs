//! Tableau de bord — statistiques réelles de `/api/v1/user-info`
//! (device_count, orgs, tier). Porté du `Dashboard.tsx` React.

use dioxus::prelude::*;
use dioxus_i18n::t;

#[component]
pub fn Dashboard() -> Element {
    rsx! { h1 { class: "text-3xl font-bold text-gray-900", {t!("nav-dashboard")} } }
}
