//! Modale générique — overlay + carte du pattern `ConfirmDialog`, corps
//! libre. `max_width` est un littéral Tailwind passé par l'appelant
//! (« max-w-md », « max-w-2xl ») pour rester visible au scan du CSS.

use dioxus::prelude::*;
use dioxus_i18n::t;

use super::icons;

/// Footer row of a dialog's actions, pinned to the bottom of the scrolling
/// [`Modal`] body so the submit stays reachable in long forms (negative
/// margins = the body padding). Must be a direct descendant of the body
/// content (no extra padded or scrolling wrapper in between). The sticky
/// offset is negative too: a sticky box stays inside the body's *content*
/// box, so `bottom-0` lifted the footer by the padding and it covered the
/// last line of a form that did not even scroll.
pub const MODAL_FOOTER: &str = "sticky -bottom-4 sm:-bottom-6 -mx-4 -mb-4 flex justify-end gap-2 border-t border-gray-100 bg-white px-4 py-3 sm:-mx-6 sm:-mb-6 sm:px-6";

#[component]
pub fn Modal(
    title: String,
    max_width: String,
    on_close: Callback<()>,
    children: Element,
) -> Element {
    rsx! {
        div {
            class: "fixed inset-0 bg-black/50 flex items-center justify-center z-50 p-3 sm:p-4",
            onclick: move |_| on_close.call(()),
            div {
                // Never taller than the viewport: header stays put, only the body
                // scrolls (a phone used to lose the dialog's buttons).
                class: "flex max-h-full w-full flex-col bg-white rounded-lg shadow-xl {max_width}",
                role: "dialog",
                aria_modal: "true",
                aria_label: "{title}",
                onclick: move |event| event.stop_propagation(),
                div { class: "flex shrink-0 items-center justify-between px-4 py-3 sm:px-6 sm:py-4 border-b border-gray-200",
                    h3 { class: "text-lg font-semibold text-gray-900", "{title}" }
                    button {
                        class: "p-1 text-gray-400 hover:text-gray-600 transition-colors",
                        r#type: "button",
                        title: t!("common-close"),
                        aria_label: t!("common-close"),
                        onclick: move |_| on_close.call(()),
                        icons::X { class: Some("w-5 h-5".into()) }
                    }
                }
                div { class: "min-h-0 flex-1 overflow-y-auto p-4 sm:p-6", {children} }
            }
        }
    }
}
