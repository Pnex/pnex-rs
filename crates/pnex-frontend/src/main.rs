//! PNEX frontend — Dioxus CSR web.
//!
//! Squelette Phase 1 : une page vide qui prouve la chaîne de build
//! (wasm32 + assets servis par Loco). L'app réelle arrive en Phase 3+.
//! Les appels API se feront via gloo-net vers le backend Loco.

use dioxus::prelude::*;

fn main() {
    dioxus::launch(App);
}

#[component]
fn App() -> Element {
    rsx! {
        // Feuille de style bundlée par manganis (hashée en build).
        link { rel: "stylesheet", href: asset!("/assets/main.css") }
        header { class: "app-header",
            h1 { "PNEX" }
            p { "Squelette Phase 1 — backend Loco opérationnel" }
        }
        main {
            // Consomme pnex-core pour prouver le partage de types wasm ↔ natif.
            p { class: "status", "{pnex_core::SERVICE_NAME} : en attente du backend" }
        }
    }
}
