//! The one map list panel (design-system.md D223): the main map and the
//! map of a case show their items in the same panel — title and close,
//! search, optional filters, a scrolling list, a footer — so both maps look
//! and behave alike. `docked` = laid over the map's right edge (a case
//! canvas, where the left corner belongs to the editor's `+`); otherwise a
//! column on the left (overlay drawer on phones).

use dioxus::prelude::*;
use dioxus_i18n::t;

use crate::components::icons;

const INPUT: &str = crate::components::ui::INPUT;

#[component]
pub fn MapPanel(
    title: String,
    /// The page heading (main map): an `h1`; a panel inside a page: `h2`.
    #[props(default)]
    page_heading: bool,
    /// Docked over the map's right edge instead of a left column.
    #[props(default)]
    docked: bool,
    /// Extra header content, before the close button (a link).
    #[props(default)]
    header_extra: Option<Element>,
    on_close: Callback<()>,
    search: Signal<String>,
    search_placeholder: String,
    /// Called on every keystroke of the search (server-filtered lists).
    #[props(default)]
    on_search: Option<Callback<()>>,
    #[props(default)] filters: Option<Element>,
    footer: Element,
    children: Element,
) -> Element {
    let mut search = search;
    let panel = if docked {
        "pnex-drawer-in absolute inset-y-0 right-0 z-20 flex w-80 max-w-[85vw] flex-col border-l border-gray-200 bg-white shadow-xl"
    } else {
        "pnex-drawer-in absolute inset-y-0 left-0 z-30 w-80 max-w-[85vw] shrink-0 bg-white border-r border-gray-200 flex flex-col shadow-xl lg:static lg:z-auto lg:max-w-none lg:shadow-none"
    };
    rsx! {
        aside { class: panel, "data-testid": "map-panel",
            div { class: "flex items-center justify-between px-4 py-3 border-b border-gray-200",
                if page_heading {
                    h1 { class: "text-base font-semibold text-gray-900", "{title}" }
                } else {
                    h2 { class: "text-base font-semibold text-gray-900", "{title}" }
                }
                // Header extras (a link) sit by the close button, on the right.
                div { class: "ml-auto flex items-center gap-1",
                    if let Some(extra) = header_extra {
                        {extra}
                    }
                    button {
                        class: "p-1.5 text-gray-500 hover:text-gray-900 rounded-lg hover:bg-gray-100 transition-colors",
                        title: t!("poi-collapse"),
                        aria_label: t!("poi-collapse"),
                        onclick: move |_| on_close.call(()),
                        icons::X { class: "h-5 w-5" }
                    }
                }
            }
            div { class: "px-4 py-3 border-b border-gray-100",
                input {
                    class: "{INPUT} w-full",
                    r#type: "search",
                    placeholder: "{search_placeholder}",
                    aria_label: "{search_placeholder}",
                    value: "{search}",
                    oninput: move |e| {
                        search.set(e.value());
                        if let Some(cb) = on_search {
                            cb.call(());
                        }
                    },
                }
            }
            if let Some(filters) = filters {
                div { class: "px-4 py-3 border-b border-gray-100 space-y-2", {filters} }
            }
            div { class: "flex-1 overflow-y-auto", {children} }
            div { class: "px-4 py-2 border-t border-gray-100 text-xs text-gray-500",
                {footer}
            }
        }
    }
}

/// One item of a [`MapPanel`] list: a leading mark, the title, an optional
/// detail line; selected = signal tint.
#[component]
pub fn MapPanelRow(
    lead: Element,
    title: String,
    #[props(default)] detail: Option<String>,
    #[props(default)] selected: bool,
    onclick: EventHandler<()>,
) -> Element {
    rsx! {
        button {
            r#type: "button",
            class: if selected { "w-full text-left px-4 py-2.5 border-b border-gray-100 bg-signal-50 transition-colors" } else { "w-full text-left px-4 py-2.5 border-b border-gray-100 hover:bg-gray-50 transition-colors" },
            onclick: move |_| onclick.call(()),
            div { class: "flex items-center gap-2",
                {lead}
                span { class: "text-sm font-medium text-gray-900 truncate flex-1", "{title}" }
            }
            if let Some(detail) = detail {
                div { class: "text-xs text-gray-500 mt-0.5 truncate", "{detail}" }
            }
        }
    }
}

/// Reopens a closed [`MapPanel`] (floating over the map).
#[component]
pub fn MapPanelToggle(onclick: EventHandler<()>) -> Element {
    rsx! {
        button {
            r#type: "button",
            class: "p-2.5 bg-white rounded-lg shadow border border-gray-200 text-gray-600 hover:text-gray-900 transition-colors",
            title: t!("poi-expand"),
            aria_label: t!("poi-expand"),
            onclick: move |_| onclick.call(()),
            icons::Menu { class: "h-5 w-5" }
        }
    }
}
