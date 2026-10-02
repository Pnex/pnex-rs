//! Symbol library panel (draw.io-like sidebar): process engineering and
//! flowchart symbols grouped by category, collapsible, with a search over
//! the localized names. A click adds a `symbol` widget to the canvas.
//! Rendered in the `tools` slot of the shell, anchored under the tools pill;
//! no backdrop (same grammar as `PalettePopover`).

use std::collections::BTreeSet;

use dioxus::prelude::*;
use dioxus_i18n::t;

use crate::components::icons;
use crate::components::symbols::{self, SymbolView, LIBRARY};

use super::library;
use super::EditorCx;

/// One thumbnail row entry, resolved at render time (localized name).
#[derive(Clone, PartialEq)]
struct Entry {
    id: &'static str,
    name: String,
}

#[component]
pub fn SymbolPanel(cx: EditorCx) -> Element {
    let mut symbols_open = cx.symbols_open;
    let mut search = use_signal(String::new);
    // Flowchart open by default: the lightest category to start with.
    let mut open_cats: Signal<BTreeSet<&'static str>> =
        use_signal(|| BTreeSet::from(["flowchart"]));
    let close_label = t!("eshell-close");
    let placeholder = t!("sym-search");

    let query = search().trim().to_lowercase();
    // Category → matching entries (names resolved once per render).
    let groups: Vec<(&'static str, String, Vec<Entry>)> = LIBRARY
        .iter()
        .map(|(cat, list)| {
            let label = symbols::category_label(cat);
            let label_match = !query.is_empty() && label.to_lowercase().contains(&query);
            let entries: Vec<Entry> = list
                .iter()
                .map(|s| Entry {
                    id: s.id,
                    name: symbols::symbol_name(s),
                })
                .filter(|e| {
                    query.is_empty()
                        || label_match
                        || e.name.to_lowercase().contains(&query)
                        || e.id.contains(&query)
                })
                .collect();
            (*cat, label, entries)
        })
        .filter(|(_, _, entries)| !entries.is_empty())
        .collect();
    let searching = !query.is_empty();
    let no_result = groups.is_empty();

    rsx! {
        div { class: "absolute left-0 top-12 z-30 flex max-h-[75vh] w-80 flex-col overflow-hidden rounded-xl border border-gray-200 bg-white shadow-xl",
            div { class: "flex items-center justify-between px-3 pt-3",
                span { class: "text-sm font-semibold text-gray-900", {t!("sym-library")} }
                button {
                    class: "text-gray-400 hover:text-gray-600",
                    title: "{close_label}",
                    onclick: move |_| symbols_open.set(false),
                    icons::X { class: "h-4 w-4" }
                }
            }
            div { class: "p-3 pb-2",
                input {
                    class: "w-full rounded-lg border border-gray-300 px-3 py-2 text-sm focus:outline-none focus:ring-2 focus:ring-blue-500",
                    placeholder: "{placeholder}",
                    value: "{search}",
                    autofocus: true,
                    oninput: move |e: Event<FormData>| search.set(e.value()),
                    onkeydown: move |e: KeyboardEvent| {
                        if e.key() == Key::Escape {
                            symbols_open.set(false);
                        }
                    },
                }
            }
            div { class: "min-h-0 flex-1 overflow-y-auto px-2 pb-2",
                for (cat, label, entries) in groups {
                    div {
                        key: "{cat}",
                        class: "border-b border-gray-100 last:border-b-0",
                        button {
                            class: "flex w-full items-center gap-1 px-1 py-1.5 text-left text-xs font-semibold text-gray-600 hover:text-gray-900",
                            onclick: move |_| {
                                open_cats
                                    .with_mut(|set| {
                                        if !set.remove(cat) {
                                            set.insert(cat);
                                        }
                                    });
                            },
                            if searching || open_cats.read().contains(cat) {
                                icons::ChevronDown { class: "h-3.5 w-3.5" }
                            } else {
                                icons::ChevronRight { class: "h-3.5 w-3.5" }
                            }
                            span { class: "truncate", "{label}" }
                            span { class: "ml-auto text-[10px] font-normal text-gray-400",
                                "{entries.len()}"
                            }
                        }
                        if searching || open_cats.read().contains(cat) {
                            div { class: "grid grid-cols-5 gap-1 pb-2",
                                for entry in entries {
                                    SymbolTile {
                                        key: "{entry.id}",
                                        cx,
                                        entry,
                                    }
                                }
                            }
                        }
                    }
                }
                if no_result {
                    p { class: "px-2 py-4 text-center text-sm text-gray-400",
                        {t!("eshell-no-result")}
                    }
                }
            }
        }
    }
}

#[component]
fn SymbolTile(cx: EditorCx, entry: Entry) -> Element {
    let id = entry.id;
    rsx! {
        button {
            class: "flex h-14 items-center justify-center rounded-md border border-transparent p-1.5 hover:border-blue-200 hover:bg-blue-50",
            title: "{entry.name}",
            "aria-label": "{entry.name}",
            onclick: move |_| library::add_symbol(cx, id),
            SymbolView { shape: id.to_string(), stroke_width: 1.2 }
        }
    }
}
