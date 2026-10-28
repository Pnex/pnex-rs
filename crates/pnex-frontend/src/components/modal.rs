//! Generic modal — overlay + card of the `ConfirmDialog` pattern, free
//! body. `max_width` is a Tailwind literal passed by the caller
//! ("max-w-md", "max-w-2xl") to stay visible to the CSS scan.

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
pub const MODAL_FOOTER: &str = "sticky -bottom-4 sm:-bottom-6 -mx-4 -mb-4 flex justify-end gap-2 border-t border-line bg-gray-50 px-4 py-3 sm:-mx-6 sm:-mb-6 sm:px-6";

#[component]
pub fn Modal(
    title: String,
    max_width: String,
    on_close: Callback<()>,
    children: Element,
) -> Element {
    rsx! {
        div {
            // Phone: a bottom sheet (thumb reach); from sm up: a centred dialog.
            class: "pnex-backdrop-in fixed inset-0 bg-ink/55 dark:bg-black/60 flex items-end sm:items-center justify-center z-50 pt-8 sm:p-4",
            onclick: move |_| on_close.call(()),
            div {
                // Never taller than the viewport: header stays put, only the body
                // scrolls (a phone used to lose the dialog's buttons).
                class: "pnex-dialog-in flex max-h-full w-full flex-col bg-surface rounded-t-xl sm:rounded-lg shadow-2xl outline-none {max_width}",
                role: "dialog",
                aria_modal: "true",
                aria_label: "{title}",
                tabindex: "-1",
                "data-pnex-modal": "",
                // Keyboard focus inside the dialog (Escape reaches it), unless
                // a field of the dialog already took it.
                onmounted: move |_| {
                    document::eval(
                        "const d=[...document.querySelectorAll('[data-pnex-modal]')].pop();if(d&&!d.contains(document.activeElement))d.focus();",
                    );
                },
                onclick: move |event| event.stop_propagation(),
                onkeydown: move |event| {
                    if event.key() == Key::Escape {
                        // Only the innermost dialog closes.
                        event.stop_propagation();
                        on_close.call(());
                    }
                },
                div { class: "flex shrink-0 items-center justify-between gap-4 px-4 py-3 sm:px-6 sm:py-4 border-b border-line",
                    h3 { class: "text-xl font-semibold text-ink", "{title}" }
                    button {
                        class: "-mr-2 inline-flex h-9 w-9 items-center justify-center rounded-md text-slate hover:bg-gray-100 hover:text-ink transition-colors",
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
