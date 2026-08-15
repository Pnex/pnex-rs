//! Organisations — nouvelle page (multi-tenant Phase 3) : liste, création,
//! sélection ; détail piloté par le signal global `ORG`.

use dioxus::prelude::*;
use dioxus_i18n::t;

#[component]
pub fn Orgs() -> Element {
    rsx! { h1 { class: "text-3xl font-bold text-gray-900", {t!("nav-orgs")} } }
}

#[component]
pub fn OrgDetail() -> Element {
    rsx! { p { {t!("common-loading")} } }
}
