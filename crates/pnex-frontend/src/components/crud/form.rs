//! Standardised modal form — `Modal` + cancel/submit footer. The fields
//! stay in `children` with their page-local signals (idiom of the
//! historical creation modals); only the footer is normalised, with the
//! same classes as the existing footers (consts shared with
//! `layout.rs`).

use dioxus::prelude::*;
use dioxus_i18n::t;

use super::layout::{GHOST_BTN, PRIMARY_BTN};
use crate::components::modal::Modal;

#[component]
pub fn FormDialog(
    /// Modal title (i18n resolved by the caller).
    title: String,
    /// Submit button label (i18n resolved by the caller).
    submit_label: String,
    /// Close (overlay, cross, cancel button).
    on_close: Callback<()>,
    /// Submit — the page handler keeps the spawn / toasts /
    /// closing / refetch.
    on_submit: Callback<()>,
    /// Server operation in progress → submit disabled.
    busy: bool,
    /// Prior validation → submit disabled (default `true`; set
    /// `false` when the page shows its error inline **after** the click,
    /// the button must not be locked before).
    #[props(default = true)]
    valid: bool,
    #[props(default = "max-w-md".to_string())] max_width: String,
    children: Element,
) -> Element {
    rsx! {
        Modal { title, max_width, on_close,
            div { class: "space-y-4",
                {children}
                div { class: crate::components::modal::MODAL_FOOTER,
                    button {
                        class: GHOST_BTN,
                        r#type: "button",
                        onclick: move |_| on_close.call(()),
                        {t!("common-cancel")}
                    }
                    button {
                        class: "{PRIMARY_BTN} disabled:opacity-40 disabled:cursor-not-allowed",
                        r#type: "button",
                        disabled: busy || !valid,
                        onclick: move |_| on_submit.call(()),
                        {submit_label}
                    }
                }
            }
        }
    }
}
