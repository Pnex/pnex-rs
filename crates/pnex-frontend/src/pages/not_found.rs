//! Route inconnue — renvoyée par le routeur (`/:..route`).

use dioxus::prelude::*;
use dioxus_i18n::t;

#[component]
pub fn NotFound(route: Vec<String>) -> Element {
    rsx! { p { {t!("not-found", path: route.join("/"))} } }
}
