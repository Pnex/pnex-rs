//! Auxiliary floating panel — today only the studio editor
//! (floor list, link mode, scene counter), rendered in the `tools` slot
//! of `EditorShell` next to the `+`. Same visual grammar as the
//! `PalettePopover` card: anchored `absolute left-0 top-12` on the slot
//! wrapper.

use dioxus::prelude::*;
use dioxus_i18n::t;

use crate::components::icons;

#[component]
pub fn FloatingPanel(
    /// Panel header ("Floors"…).
    title: String,
    /// Fermeture (croix).
    on_close: Callback<()>,
    /// Body (floor list, studio tools…).
    children: Element,
) -> Element {
    let close_label = t!("eshell-close");
    rsx! {
        div { class: "pnex-pop-in absolute left-0 top-12 z-30 w-44 rounded-lg border border-gray-200 bg-white p-3 shadow-xl",
            div { class: "flex items-center justify-between",
                span { class: "text-xs font-semibold text-gray-500", {title} }
                button {
                    class: "text-gray-500 hover:text-gray-600",
                    title: "{close_label}",
                    onclick: move |_| on_close.call(()),
                    icons::X { class: "h-3.5 w-3.5" }
                }
            }
            div { class: "mt-2 max-h-72 space-y-2 overflow-y-auto", {children} }
        }
    }
}
