//! Panneau flottant annexe — aujourd'hui uniquement l'éditeur studio
//! (liste des étages, mode liaison, compteur de scènes), rendu dans le slot
//! `tools` de `EditorShell` à côté du `+`. Même grammaire visuelle que la
//! carte de `PalettePopover` : ancré `absolute left-0 top-12` sur le wrapper
//! du slot.

use dioxus::prelude::*;
use dioxus_i18n::t;

use crate::components::icons;

#[component]
pub fn FloatingPanel(
    /// En-tête du panneau (« Étages »…).
    title: String,
    /// Fermeture (croix).
    on_close: Callback<()>,
    /// Corps (liste des étages, outils du studio…).
    children: Element,
) -> Element {
    let close_label = t!("eshell-close");
    rsx! {
        div { class: "absolute left-0 top-12 z-30 w-44 rounded-lg border border-gray-200 bg-white p-3 shadow-xl",
            div { class: "flex items-center justify-between",
                span { class: "text-xs font-semibold uppercase tracking-wide text-gray-500", {title} }
                button {
                    class: "text-gray-400 hover:text-gray-600",
                    title: "{close_label}",
                    onclick: move |_| on_close.call(()),
                    icons::X { class: "h-3.5 w-3.5" }
                }
            }
            div { class: "mt-2 max-h-72 space-y-2 overflow-y-auto", {children} }
        }
    }
}
