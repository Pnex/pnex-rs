//! Inspecteur divulgatif (principe 4) — le configurateur n'existe qu'au clic
//! sur un élément : glisse depuis la droite par-dessus le canvas, se ferme à
//! la croix ou Échap. Le corps du formulaire reste la propriété de l'éditeur
//! (`body: Element`) ; la coquille fournit l'en-tête (icône + nom + réf +
//! croix) et le socle du panneau.

use dioxus::prelude::*;
use dioxus_i18n::t;

use super::palette::{IconView, PaletteIcon};
use crate::components::icons;

/// Panneau inspecteur — `absolute inset-y-0 right-0` dans le corps de
/// `EditorShell`. Échap sur le panneau (ou bubbling depuis un champ du
/// formulaire) ferme via `on_close` ; la désélection par le canvas ferme
/// aussi (None ⇒ panneau absent).
#[component]
pub fn InspectorPanel(
    /// Icône du type édité (tuile d'en-tête).
    #[props(default)]
    icon: Option<PaletteIcon>,
    /// Nom de l'élément (ou label du type si le nom est vide).
    title: String,
    /// Référence annexe (« #N1 »).
    #[props(default)]
    subtitle: Option<String>,
    /// Fermeture (croix / Échap) — l'éditeur désélectionne.
    on_close: Callback<()>,
    /// Corps du formulaire, fourni par l'éditeur.
    body: Element,
) -> Element {
    let close_label = t!("eshell-close");
    rsx! {
        aside {
            class: "absolute inset-y-0 right-0 z-30 flex h-full w-80 flex-col border-l border-gray-200 bg-white shadow-xl",
            tabindex: "0",
            onkeydown: move |e: KeyboardEvent| {
                if e.key() == Key::Escape {
                    on_close.call(());
                }
            },
            div { class: "flex items-center gap-2.5 border-b border-gray-200 px-4 py-3",
                {match icon {
                    Some(icon) => rsx! {
                        span { class: "flex h-8 w-8 shrink-0 items-center justify-center rounded-lg bg-blue-50 text-blue-600",
                            IconView { icon: icon, class: "h-4 w-4" }
                        }
                    },
                    None => rsx! {},
                }}
                div { class: "min-w-0",
                    div { class: "truncate text-sm font-semibold text-gray-900", {title} }
                    if let Some(sub) = subtitle {
                        div { class: "text-xs text-gray-400", {sub} }
                    }
                }
                div { class: "flex-1" }
                button {
                    class: "shrink-0 rounded-lg p-1.5 text-gray-400 hover:bg-gray-100 hover:text-gray-600",
                    title: "{close_label}",
                    onclick: move |_| on_close.call(()),
                    icons::X { class: "h-4 w-4" }
                }
            }
            div { class: "flex-1 space-y-3 overflow-y-auto p-4", {body} }
        }
    }
}
