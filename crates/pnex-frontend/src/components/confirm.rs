//! Confirmation modal (i18n on the caller side) — replaces the browser
//! `confirm()` calls of the original React UI.

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
            class: "pnex-backdrop-in fixed inset-0 bg-ink/55 dark:bg-black/60 flex items-end sm:items-center justify-center z-50 sm:p-4",
            onclick: move |_| on_cancel.call(()),
            div {
                class: "pnex-dialog-in w-full max-w-md overflow-hidden rounded-t-xl sm:rounded-lg bg-surface shadow-2xl",
                role: "alertdialog",
                aria_modal: "true",
                aria_label: "{title}",
                onclick: move |event| event.stop_propagation(),
                onkeydown: move |event| {
                    if event.key() == Key::Escape {
                        // Only the innermost dialog closes.
                        event.stop_propagation();
                        on_cancel.call(());
                    }
                },
                div { class: "space-y-3 p-6",
                    h3 { class: "text-xl font-semibold text-ink", "{title}" }
                    p { class: "text-sm text-slate", {message} }
                }
                div { class: "flex justify-end gap-2 border-t border-line bg-gray-50 px-6 py-4",
                    button {
                        class: crate::components::ui::BTN_SECONDARY,
                        r#type: "button",
                        // Focus on the safe answer: Escape and Enter both cancel.
                        onmounted: move |e| async move {
                            let _ = e.set_focus(true).await;
                        },
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
