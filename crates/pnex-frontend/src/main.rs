//! PNEX frontend — Dioxus CSR web, servi en statique par le backend Loco
//! (same-origin : URLs API relatives, pas de CORS).
//!
//! Socle : i18n Fluent (fr-FR/en-US) + routeur statique. La feuille de style
//! est le CSS Tailwind v4 généré (`bun run css:build`, Taskfile) — pattern
//! manganis `asset!()` hérité de la Phase 1.

// Windows release: GUI subsystem, no console window behind the app.
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod api;
mod app;
mod auth;
mod capture;
mod capture360;
mod components;
#[cfg(not(target_arch = "wasm32"))]
mod download;
mod flash;
mod i18n;
mod i18n_guard;
mod js_guard;
mod map_viewer;
mod media_cache;
mod media_viewer;
mod pages;
mod state;
mod storage;
mod surface_guard;
mod tour_viewer;
mod tron;
mod util;
mod version;

use crate::app::Route;
use dioxus::prelude::*;

/// CSS inline natif : l'APK n'embarque pas de shell HTML avec `<link>` (dx ne
/// l'injecte que pour le build web) — sans style inline, le premier patch DOM
/// peint une frame non stylée puis se restyle (FOUC constaté sur émulateur).
/// Sur web la constante est vide : le `<link>` index.html suffit.
#[cfg(not(target_arch = "wasm32"))]
static CSS_INLINE: &str = include_str!("../assets/tailwind.css");
#[cfg(target_arch = "wasm32")]
static CSS_INLINE: &str = "";
// CSS des viewers média (contrôles pannellum) — même logique d'inlining
// natif que tailwind.css (l'APK n'a pas de shell avec <link>).
#[cfg(not(target_arch = "wasm32"))]
static VIEWERS_CSS_INLINE: &str = include_str!("../assets/viewers.css");
// CSS de maplibre (position:absolute des markers, contrôles, attribution) —
// généré par js:map (import CSS dans js/map.js). SANS lui, les markers HTML
// s'empilent sous le canvas (flux normal) et deviennent invisibles.
static MAP_CSS_INLINE: &str = include_str!("../assets/map.css");
#[cfg(target_arch = "wasm32")]
static VIEWERS_CSS_INLINE: &str = "";

fn main() {
    // Hook de panic : le message standard stderr est perdu quand l'abort
    // JNI (panic_cannot_unwind dans handleRequest) survient avant le flush —
    // on trace message + localisation explicitement (visible en logcat).
    #[cfg(target_os = "android")]
    std::panic::set_hook(Box::new(|info| {
        eprintln!(
            "PNEX-PANIC: {} @ {}:{}:{}",
            info,
            std::thread::current().name().unwrap_or("?"),
            info.location().map(|l| l.file()).unwrap_or("?"),
            info.location().map(|l| l.line()).unwrap_or(0),
        );
    }));
    // Logger Android : sans init, les `log::warn!` (capture360, pipeline)
    // partent dans le vide — et le smoke test des signes capteur a besoin
    // des poses visibles dans logcat.
    #[cfg(target_os = "android")]
    {
        let filter = std::env::var("PNEX_LOG").unwrap_or_else(|_| "info".into());
        android_logger::init_once(
            android_logger::Config::default()
                .with_max_level(filter.parse().unwrap_or(log::LevelFilter::Info))
                .with_tag("pnex"),
        );
    }
    dioxus::launch(App);
}

#[component]
fn App() -> Element {
    i18n::init();
    // Restauration de session (tokens → user-info) une seule fois au boot.
    use_hook(|| {
        spawn(async {
            state::session::boot().await;
        });
    });
    // Porte de compatibilité app ↔ serveur (contrat d'API) : no-op tant
    // qu'aucune base n'est résolue (premier lancement natif). Re-déclenchée
    // par `apply_server` à chaque changement de serveur.
    use_hook(|| {
        spawn(async {
            state::compat::check().await;
        });
    });
    // Cible native sans URL serveur résolue : l'écran de configuration
    // remplace le routeur au premier lancement (façon Bitwarden). Global :
    // la page de login (logout → « Changer de serveur ») rouvre cet écran.
    // Sur web (same-origin) SERVER_READY est toujours vrai — cf. api::config.
    rsx! {
        // Favicon : mark X seul — lisible en 32 px, contrairement au wordmark.
        link {
            rel: "icon",
            r#type: "image/png",
            href: asset!("/assets/logo-mark.png"),
        }
        // CSS : sur web le <link> est déjà dans index.html (dx build, manganis)
        // et bloque le rendu. Sur natif l'APK n'embarque pas de shell avec
        // <link> — sans style inline, le premier patch DOM peint une frame non
        // stylée avant l'arrivée du CSS (FOUC constaté sur émulateur Android,
        // 2026-09-08). include_str! exige le CSS réel (task css:build en deps).
        style { {CSS_INLINE} }
        link { rel: "stylesheet", href: asset!("/assets/tailwind.css") }
        // Glue esptool-js (flash navigateur, Web Serial) — script classique
        // IIFE qui expose window.pnexFlash/pnexFlashSupported (cf. flash.rs).
        // Consommé au clic sur « Flasher », aucun souci d'ordre de chargement.
        script { src: asset!("/assets/flasher.js") }
        // Fond animé de la page de login (gerbe de faisceaux WebGL2) —
        // IIFE qui expose window.pnexTronGerbe (cf. tron.rs, login.rs).
        script { src: asset!("/assets/tron-gerbe.js") }
        // Viewers WebGL média (pannellum + gsplat.js) — IIFE qui expose
        // window.pnexViewers (cf. media_viewer.rs). CSS pannellum requis
        // (contrôles) ; inline en natif, <link> sur web.
        script { src: asset!("/assets/viewers.js") }
        // Carte géo (D27) — maplibre-gl 4.x, style Protomaps auto-hébergé.
        script { src: asset!("/assets/map.js") }
        style { {VIEWERS_CSS_INLINE} }
        link { rel: "stylesheet", href: asset!("/assets/viewers.css") }
        style { {MAP_CSS_INLINE} }
        link { rel: "stylesheet", href: asset!("/assets/map.css") }
        if !pages::server_url::SERVER_READY() {
            pages::server_url::ServerUrl {}
        } else if state::compat::GATE.cloned() == state::compat::GateState::Ok {
            // Serveur conforme et même contrat d'API.
            Router::<Route> {}
        } else {
            // Vérification en cours, serveur injoignable ou contrat
            // différent : écran bloquant (refus propre).
            pages::gate::GateScreen {}
        }
        // Root CA confirmation (native, D70) — overlay above any screen.
        pages::trust_ca::TrustCaDialog {}
    }
}
