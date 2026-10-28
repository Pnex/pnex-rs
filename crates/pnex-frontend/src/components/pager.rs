//! Reusable pager — navigates a 0-based page signal over a server-paginated
//! list (`{count, …}` D14 envelope). Hidden if there is a single page.

use crate::components::ui::BTN_SECONDARY_SM;
use dioxus::prelude::*;
use dioxus_i18n::t;

#[component]
pub fn Pager(
    /// Total returned by the envelope.
    count: i64,
    /// Page size of the list.
    page_size: i64,
    /// Signal of the current page (0-based).
    page: Signal<i64>,
    on_navigate: Callback<i64>,
) -> Element {
    // Ceiling division (signed div_ceil unstable on this toolchain).
    let pages = (count + page_size - 1) / page_size;
    if pages <= 1 {
        return rsx! {};
    }
    let current = page().clamp(0, pages - 1);
    let prev = current.saturating_sub(1);
    let next = (current + 1).min(pages - 1);

    rsx! {
        div { class: "mt-4 flex items-center justify-center gap-3 text-sm",
            button {
                class: BTN_SECONDARY_SM,
                disabled: current == 0,
                onclick: move |_| on_navigate.call(prev),
                {t!("pagination-previous")}
            }
            span { class: "text-gray-600",
                "Page {current + 1} / {pages}"
                span { class: "text-gray-500 ml-2", "({count})" }
            }
            button {
                class: BTN_SECONDARY_SM,
                disabled: current >= pages - 1,
                onclick: move |_| on_navigate.call(next),
                {t!("pagination-next")}
            }
        }
    }
}
