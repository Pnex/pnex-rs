//! Catalogue d'appareils — porté du `Catalog.tsx` React. Les endpoints
//! predefined-devices arrivent en Phase 4 : page en attente.

use dioxus::prelude::*;
use dioxus_i18n::t;

use crate::components::empty_state::{EmptyState, package_icon};

#[component]
pub fn Catalog() -> Element {
    rsx! {
        div { class: "p-6",
            div { class: "mb-8",
                h1 { class: "text-3xl font-bold text-gray-900", {t!("nav-catalog")} }
                p { class: "text-gray-600 mt-2", {t!("empty-catalog-message")} }
            }
            EmptyState {
                icon: package_icon(),
                title: t!("empty-catalog-title"),
                message: t!("empty-catalog-message"),
                phase: "4".to_string(),
            }
        }
    }
}
