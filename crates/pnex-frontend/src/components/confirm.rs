//! Modale de confirmation (i18n côté appelant) — remplace les `confirm()`
//! navigateur de l'UI React d'origine.

use dioxus::prelude::*;
use dioxus_i18n::t;

#[component]
pub fn ConfirmDialog(
    title: String,
    message: String,
    confirm_label: String,
    on_confirm: Callback<()>,
    on_cancel: Callback<()>,
) -> Element {
    rsx! {
        div {
            class: "fixed inset-0 bg-black bg-opacity-50 flex items-center justify-center z-50 p-4",
            onclick: move |_| on_cancel.call(()),
            div {
                class: "bg-white rounded-lg shadow-xl max-w-md w-full p-6 space-y-4",
                onclick: move |event| event.stop_propagation(),
                h3 { class: "text-lg font-semibold text-gray-900", {title} }
                p { class: "text-sm text-gray-600", {message} }
                div { class: "flex justify-end space-x-3 pt-2",
                    button {
                        class: "px-4 py-2 text-sm text-gray-600 hover:text-gray-900 transition-colors",
                        onclick: move |_| on_cancel.call(()),
                        {t!("common-cancel")}
                    }
                    button {
                        class: "px-4 py-2 text-sm font-semibold text-white bg-red-600 rounded-lg hover:bg-red-700 transition-colors",
                        onclick: move |_| on_confirm.call(()),
                        {confirm_label}
                    }
                }
            }
        }
    }
}
