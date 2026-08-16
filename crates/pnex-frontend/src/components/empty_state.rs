//! Empty-state « page en attente de phase » — les pages Devices/Builds/
//! Catalog sont portées visuellement mais leurs endpoints arrivent en
//! Phase 4 (devices, catalogue) et 6 (builds firmware).

use dioxus::prelude::*;
use dioxus_i18n::t;

use crate::components::icons;

#[component]
pub fn EmptyState(icon: Element, title: String, message: String, phase: String) -> Element {
    rsx! {
        div { class: "bg-white rounded-lg shadow-sm",
            div { class: "p-12 text-center",
                div { class: "p-4 bg-gray-100 rounded-full w-16 h-16 mx-auto mb-4 flex items-center justify-center",
                    {icon}
                }
                h3 { class: "text-xl font-semibold text-gray-900 mb-2", {title} }
                p { class: "text-gray-600", {message} }
                span { class: "inline-flex items-center mt-4 px-3 py-1 rounded-full text-xs font-semibold bg-blue-50 text-blue-700 border border-blue-200",
                    {t!("empty-phase", phase: phase)}
                }
            }
        }
    }
}

/// Icônes courantes des empty-states (passées en prop d'`EmptyState`).
pub fn wrench_icon() -> Element {
    rsx! { icons::Wrench { class: "h-8 w-8 text-gray-400" } }
}

pub fn package_icon() -> Element {
    rsx! { icons::Package { class: "h-8 w-8 text-gray-400" } }
}
