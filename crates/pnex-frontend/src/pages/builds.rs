//! Builds firmware — portée du `Builds.tsx` React. Les endpoints de build
//! arrivent en Phase 6 : page en attente (empty-state).

use dioxus::prelude::*;
use dioxus_i18n::t;

#[component]
pub fn Builds() -> Element {
    rsx! { h1 { class: "text-3xl font-bold text-gray-900", {t!("nav-builds")} } }
}
