//! Gestion des appareils — portée du `Devices.tsx` React. Les endpoints
//! devices arrivent en Phase 4 : page en attente (empty-state).

use dioxus::prelude::*;
use dioxus_i18n::t;

use crate::components::empty_state::{EmptyState, cpu_icon};

#[component]
pub fn Devices() -> Element {
    rsx! {
        div { class: "p-6",
            div { class: "mb-8",
                h1 { class: "text-3xl font-bold text-gray-900", {t!("nav-devices")} }
                p { class: "text-gray-600 mt-2", {t!("empty-devices-message")} }
            }
            EmptyState {
                icon: cpu_icon(),
                title: t!("empty-devices-title"),
                message: t!("empty-devices-message"),
                phase: "4".to_string(),
            }
        }
    }
}
