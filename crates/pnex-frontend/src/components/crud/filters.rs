//! Barre de filtres canonique — le conteneur et les deux champs dupliqués
//! verbatim dans les pages historiques (search + refresh). Les champs
//! spécifiques (selects i18n de la page) restent en rsx page, posés dans le
//! `FilterBar`.

use dioxus::prelude::*;
use dioxus_i18n::t;

use crate::components::icons;

/// Conteneur des filtres — même classe que le slot `filters` de
/// [`super::layout::ListLayout`], pour poser la barre dans le corps de page
/// (filtres propres à la branche liste, masqués dans les sous-vues).
#[component]
pub fn FilterBar(children: Element) -> Element {
    rsx! {
        div { class: "mb-6 flex flex-wrap items-center gap-2", {children} }
    }
}

/// Champ de recherche serveur — saisie **live sans refetch** (le signal
/// suffit) ; la soumission est explicite : Enter → `prevent_default` +
/// `on_submit` (côté page : reset de la page courante + reload). Les
/// nouveaux filtres remettent toujours la page à 0 avant le refetch.
#[component]
pub fn SearchInput(
    /// Placeholder (i18n résolu par l'appelant).
    placeholder: String,
    /// Signal de la requête courante (saisie conservée entre deux rendus).
    mut value: Signal<String>,
    on_submit: Callback<()>,
) -> Element {
    rsx! {
        input {
            class: "flex-1 min-w-48 px-3 py-2 border border-gray-300 rounded-lg text-sm",
            r#type: "search",
            placeholder: "{placeholder}",
            value: "{value}",
            oninput: move |event| value.set(event.value()),
            onkeydown: move |event| {
                if event.key() == Key::Enter {
                    event.prevent_default();
                    on_submit.call(());
                }
            },
        }
    }
}

/// Manual refresh button — icon-only everywhere (single idiom).
/// `inline-flex items-center`: an inline SVG sits on the text baseline and
/// otherwise inflates the line box by ~5 px (button taller than neighboring
/// selects). Icon `h-5 w-5` → same 38 px height as `py-2 text-sm` inputs.
#[component]
pub fn RefreshButton(on_click: Callback<()>) -> Element {
    rsx! {
        button {
            class: "inline-flex items-center justify-center shrink-0 px-3 py-2 text-sm text-gray-600 border border-gray-300 rounded-lg hover:bg-gray-50 transition-colors",
            r#type: "button",
            title: t!("common-refresh"),
            "aria-label": t!("common-refresh"),
            onclick: move |_| on_click.call(()),
            icons::RefreshCw { class: "h-5 w-5" }
        }
    }
}
