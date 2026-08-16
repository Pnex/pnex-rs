//! Builds firmware — portée du `Builds.tsx` React. Les endpoints de build
//! arrivent en Phase 6 (worker firmware-builder) : page en attente.

use dioxus::prelude::*;
use dioxus_i18n::t;

use crate::components::empty_state::{EmptyState, wrench_icon};

#[component]
pub fn Builds() -> Element {
    rsx! {
        div { class: "p-6",
            div { class: "mb-8",
                h1 { class: "text-3xl font-bold text-gray-900", {t!("nav-builds")} }
                p { class: "text-gray-600 mt-2", {t!("empty-builds-message")} }
            }
            EmptyState {
                icon: wrench_icon(),
                title: t!("empty-builds-title"),
                message: t!("empty-builds-message"),
                phase: "6".to_string(),
            }
        }
    }
}
