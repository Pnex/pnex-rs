//! Modale générique — overlay + carte du pattern `ConfirmDialog`, corps
//! libre. `max_width` est un littéral Tailwind passé par l'appelant
//! (« max-w-md », « max-w-2xl ») pour rester visible au scan du CSS.

use dioxus::prelude::*;

use super::icons;

#[component]
pub fn Modal(
    title: String,
    max_width: String,
    on_close: Callback<()>,
    children: Element,
) -> Element {
    rsx! {
        div {
            class: "fixed inset-0 bg-black bg-opacity-50 flex items-center justify-center z-50 p-4",
            onclick: move |_| on_close.call(()),
            div {
                class: "bg-white rounded-lg shadow-xl w-full {max_width}",
                onclick: move |event| event.stop_propagation(),
                div {
                    class: "flex items-center justify-between px-6 py-4 border-b border-gray-200",
                    h3 { class: "text-lg font-semibold text-gray-900", {title} }
                    button {
                        class: "p-1 text-gray-400 hover:text-gray-600 transition-colors",
                        r#type: "button",
                        onclick: move |_| on_close.call(()),
                        icons::X { class: Some("w-5 h-5".into()) }
                    }
                }
                div { class: "p-6 max-h-[85vh] overflow-y-auto", {children} }
            }
        }
    }
}
