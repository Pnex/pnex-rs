//! Standardised state zones of a list — loading / empty / error, the
//! match duplicated verbatim in every historical page. Deliberately
//! **not generic**: `state` only carries the discriminant
//! (`Option<Result<(), ApiError>>`) — the `children` are built by
//! the page from its synchronous read of the resource, a `T` payload
//! would bring nothing and would force `PartialEq` on every list type.
//!
//! The page's side (`use_resource` doctrine: reads in the
//! **synchronous** part of the render body):
//!
//! ```ignore
//! let (state, is_empty, count, rows) = match &*list.value().read() {
//!     None => (None, false, 0, Vec::new()),
//!     Some(Ok(paged)) => (Some(Ok(())), paged.results.is_empty() && paged.count == 0,
//!                         paged.count, paged.results.clone()),
//!     Some(Err(err)) => (Some(Err(err.clone())), false, 0, Vec::new()),
//! };
//! ```

use dioxus::prelude::*;

use crate::api::error::ApiError;
use crate::components::gerbe::Gerbe;

#[component]
pub fn ListStates(
    /// Resource discriminant — `None` = loading,
    /// `Some(Err)` = error (server message shown as is),
    /// `Some(Ok)` = data (empty → `empty_message`, otherwise `children`).
    state: Option<Result<(), ApiError>>,
    /// Empty list (only read in `Some(Ok)`).
    is_empty: bool,
    /// Empty message — i18n resolved by the caller (the page's `*-empty` key).
    empty_message: String,
    /// Optional icon above the message (grey circle, catalog idiom).
    #[props(default)]
    empty_icon: Option<Element>,
    /// Optional detail under the empty message (catalogue hint).
    #[props(default)]
    empty_detail: Option<Element>,
    children: Element,
) -> Element {
    match state {
        None => rsx! {
            div { class: "flex justify-center py-12",
                Gerbe { size: 48 }
            }
        },
        Some(Err(err)) => rsx! {
            div {
                class: "rounded-lg border border-red-200 bg-red-50 p-4 text-sm text-alarm",
                role: "alert",
                {err.message.clone()}
            }
        },
        // The one empty-state form: what is missing, then what it is for.
        Some(Ok(())) if is_empty => rsx! {
            div { class: "flex flex-col items-center gap-2 rounded-lg border border-dashed border-line-strong px-6 py-12 text-center",
                if let Some(icon) = empty_icon {
                    div { class: "mb-2 flex h-12 w-12 items-center justify-center rounded-full bg-gray-100 text-slate",
                        {icon}
                    }
                }
                p { class: "max-w-prose text-base font-medium text-ink", {empty_message} }
                if let Some(detail) = empty_detail {
                    div { class: "max-w-prose text-sm text-slate", {detail} }
                }
            }
        },
        Some(Ok(())) => rsx! {
            {children}
        },
    }
}
