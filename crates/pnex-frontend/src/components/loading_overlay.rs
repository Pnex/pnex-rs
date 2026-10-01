//! Overlay de chargement léger et réutilisable — la variante minimale de
//! l'animation de l'écran de login (pas de traits animés : un voile
//! translucide + un spinner + un message court). À poser dans un parent
//! `relative` pour couvrir une carte/zone ; pensé pour servir sur toutes
//! les pages (opérations qui attendent le serveur : delete de flow,
//! déploiements, builds…).

use dioxus::prelude::*;

/// Voile translucide + spinner centré, message optionnel. Classes
/// littérales complètes (scan Tailwind) — opacité en syntaxe slash v4.
#[component]
pub fn LoadingOverlay(
    /// Message affiché à côté du spinner (vide = spinner seul).
    #[props(default)]
    message: String,
) -> Element {
    rsx! {
        div { class: "absolute inset-0 z-40 flex items-center justify-center bg-white/60 backdrop-blur-[2px] rounded-lg",
            div { class: "flex flex-col items-center gap-2",
                span { class: "animate-spin inline-block rounded-full h-6 w-6 border-2 border-blue-600 border-t-transparent" }
                if !message.is_empty() {
                    span { class: "text-sm text-gray-600", {message} }
                }
            }
        }
    }
}
