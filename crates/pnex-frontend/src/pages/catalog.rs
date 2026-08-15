//! Catalogue d'appareils — porté du `Catalog.tsx` React. Les endpoints
//! predefined-devices arrivent en Phase 4 : page en attente (empty-state).

use dioxus::prelude::*;
use dioxus_i18n::t;

#[component]
pub fn Catalog() -> Element {
    rsx! { h1 { class: "text-3xl font-bold text-gray-900", {t!("nav-catalog")} } }
}
