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
///
/// Below `sm` the search takes a full row of its own (first) and the page's
/// selects share the next rows evenly (min 8rem each, wrapping cleanly),
/// instead of wrapping at random with a truncated placeholder.
#[component]
pub fn FilterBar(children: Element) -> Element {
    rsx! {
        div { class: FILTER_BAR_CLASS, {children} }
    }
}

/// Container classes of [`FilterBar`] (also the `filters` slot of
/// [`super::layout::ListLayout`]).
pub const FILTER_BAR_CLASS: &str = "mb-4 md:mb-6 flex flex-wrap items-center gap-2 [&>select]:min-w-0 [&>select]:grow [&>select]:basis-32 sm:[&>select]:grow-0 sm:[&>select]:basis-auto sm:[&>select]:w-auto";

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
            class: "order-first w-full sm:order-none sm:w-auto sm:flex-1 sm:min-w-48 rounded-md border border-line-strong bg-surface px-3 py-2 text-sm text-ink placeholder:text-gray-500 focus:border-signal focus:outline-none focus:ring-3 focus:ring-signal-200",
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

/// Label filter (`name` or `name:value`) with the org's labels suggested.
#[component]
pub fn LabelFilterInput(
    placeholder: String,
    value: Signal<String>,
    on_submit: Callback<()>,
) -> Element {
    let mut value = value;
    rsx! {
        crate::components::labels_editor::LabelSuggestions { id: "pnex-labels-filter" }
        input {
            class: "order-first w-full sm:order-none sm:w-auto sm:flex-1 sm:min-w-48 rounded-md border border-line-strong bg-surface px-3 py-2 text-sm text-ink placeholder:text-gray-500 focus:border-signal focus:outline-none focus:ring-3 focus:ring-signal-200",
            r#type: "search",
            list: "pnex-labels-filter",
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
            class: "inline-flex h-10 w-10 shrink-0 items-center justify-center rounded-md border border-line-strong bg-surface text-ink hover:bg-gray-50 transition-colors",
            r#type: "button",
            title: t!("common-refresh"),
            "aria-label": t!("common-refresh"),
            onclick: move |_| on_click.call(()),
            icons::RefreshCw { class: "h-5 w-5" }
        }
    }
}
