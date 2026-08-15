//! Gestion des appareils — portée du `Devices.tsx` React. Les endpoints
//! devices arrivent en Phase 4 : page en attente (empty-state).

use dioxus::prelude::*;
use dioxus_i18n::t;

#[component]
pub fn Devices() -> Element {
    rsx! { h1 { class: "text-3xl font-bold text-gray-900", {t!("nav-devices")} } }
}
