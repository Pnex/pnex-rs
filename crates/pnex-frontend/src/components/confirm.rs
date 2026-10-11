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
            class: "fixed inset-0 bg-ink/55 dark:bg-black/60 flex items-end sm:items-center justify-center z-50 sm:p-4",
            onclick: move |_| on_cancel.call(()),
            div {
                class: "w-full max-w-md overflow-hidden rounded-t-xl sm:rounded-lg bg-surface shadow-2xl",
                role: "alertdialog",
                aria_modal: "true",
                aria_label: "{title}",
                onclick: move |event| event.stop_propagation(),
                div { class: "space-y-3 p-6",
                    h3 { class: "text-xl font-semibold text-ink", "{title}" }
                    p { class: "text-sm text-slate", {message} }
                }
                div { class: "flex justify-end gap-2 border-t border-line bg-gray-50 px-6 py-4",
                    button {
                        class: crate::components::ui::BTN_SECONDARY,
                        r#type: "button",
                        onclick: move |_| on_cancel.call(()),
                        {t!("common-cancel")}
                    }
                    button {
                        class: crate::components::ui::BTN_DANGER_SOLID,
                        r#type: "button",
                        onclick: move |_| on_confirm.call(()),
                        {confirm_label}
                    }
                }
            }
        }
    }
}
