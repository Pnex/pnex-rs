//! PNEX frontend — Dioxus CSR web, servi en statique par le backend Loco
//! (same-origin : URLs API relatives, pas de CORS).
//!
//! Socle : i18n Fluent (fr-FR/en-US) + routeur statique. La feuille de style
//! est le CSS Tailwind v4 généré (`npm run css:build`, Taskfile) — pattern
//! manganis `asset!()` hérité de la Phase 1.

mod app;
mod i18n;
mod pages;
mod storage;

use crate::app::Route;
use dioxus::prelude::*;

fn main() {
    dioxus::launch(App);
}

#[component]
fn App() -> Element {
    i18n::init();
    rsx! {
        link { rel: "stylesheet", href: asset!("/assets/tailwind.css") }
        Router::<Route> {}
    }
}
